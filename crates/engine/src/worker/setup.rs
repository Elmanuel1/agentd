//! Getting a session's sandbox ready (uploads, skills, writable workspace
//! and outputs), and copying `/mnt/session/outputs/` back as session files.

use std::collections::{BTreeMap, HashMap};

use agents::types::SkillRef;
use agents::AgentSnapshot;
use serde::{Deserialize, Serialize};
use sessions::types::{Session, SessionResource};
use sha2::{Digest, Sha256};
use shared::ports::sandbox::{ExecRequest, Owner, SandboxFile};
use shared::ApiError;

use super::SessionRun;
use crate::tools::sandbox::WORKDIR;
use crate::turn::prompt::{skill_description, SkillInfo, SKILLS_DIR};

/// Where session file resources are mounted.
pub const UPLOADS: &str = "/mnt/session/uploads";
/// Files written here become session-scoped files.
pub const OUTPUTS: &str = "/mnt/session/outputs";
/// agentd's own bookkeeping in the sandbox, owned by root.
const STATE: &str = "/mnt/session/.agentd";

/// Where a file resource lands: its `mount_path` under the uploads
/// directory, or `<uploads>/<file_id>` for the default mount.
pub fn upload_path(file_id: &str, mount_path: &str) -> String {
    if mount_path == format!("/workspace/{file_id}") {
        return format!("{UPLOADS}/{file_id}");
    }
    format!("{UPLOADS}/{}", mount_path.trim_start_matches('/'))
}

/// What the last sync stored for each output path.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Synced {
    files: BTreeMap<String, (String, String)>,
}

fn err(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(format!("sandbox: {e}"))
}

impl SessionRun {
    /// Each agent's skills, by agent name; unpacks them, mounts the uploads
    /// and makes the writable directories the first time the sandbox is used.
    pub(super) async fn prepare(
        &self,
        session: &Session,
        agents: &[&AgentSnapshot],
    ) -> Result<HashMap<String, Vec<SkillInfo>>, ApiError> {
        let target = session.id.as_str();
        let ctx = self.ctx.clone();
        let marker = format!("{STATE}/provisioned");
        let probe = ExecRequest::new(["test", "-f", marker.as_str()], std::time::Duration::from_secs(30));
        let provisioned = self.engine.sandbox.exec(target, probe).await.map_err(err)?.exit_code == 0;
        let mut put = Vec::new();
        let mut skills = HashMap::new();
        let mut unpacked = HashMap::new();
        for agent in agents {
            let mut infos = Vec::new();
            for skill in &agent.config.skills {
                let (id, version) = match skill {
                    SkillRef::Anthropic { skill_id, version } | SkillRef::Custom { skill_id, version } => {
                        (skill_id, version)
                    }
                };
                let files = match unpacked.get(id) {
                    Some(f) => f,
                    None => unpacked
                        .entry(id.clone())
                        .or_insert(self.engine.assets.skill(&ctx, id, version).await?),
                };
                let md = files
                    .files
                    .iter()
                    .find(|(p, _)| p == "SKILL.md")
                    .map(|(_, b)| String::from_utf8_lossy(b).into_owned());
                infos.push(SkillInfo {
                    name: files.name.clone(),
                    description: skill_description(&md.unwrap_or_default()),
                });
            }
            skills.insert(agent.config.name.clone(), infos);
        }
        if provisioned {
            return Ok(skills);
        }
        for files in unpacked.values() {
            for (rel, bytes) in &files.files {
                put.push(SandboxFile {
                    path: format!("{SKILLS_DIR}/{}/{rel}", files.name),
                    bytes: bytes.clone(),
                    mode: 0o644,
                });
            }
        }
        for r in &session.resources {
            if let SessionResource::File { file_id, mount_path, .. } = r {
                let bytes = self.engine.assets.file(&ctx, file_id).await?;
                put.push(SandboxFile { path: upload_path(file_id, mount_path), bytes, mode: 0o444 });
            }
        }
        self.engine.sandbox.make_dir(target, WORKDIR, Owner::Agent).await.map_err(err)?;
        self.engine.sandbox.make_dir(target, OUTPUTS, Owner::Agent).await.map_err(err)?;
        put.push(SandboxFile { path: marker, bytes: Vec::new(), mode: 0o600 });
        self.engine.sandbox.put_files(target, put).await.map_err(err)?;
        Ok(skills)
    }

    /// Copy outputs that changed since the last sync into session files,
    /// one file per path (a rewritten output replaces its earlier file).
    pub(super) async fn sync_outputs(&self, session: &str) -> Result<usize, ApiError> {
        let state_path = format!("{STATE}/outputs.json");
        let state = self.engine.sandbox.read_tree(session, STATE).await.map_err(err)?;
        let mut synced: Synced =
            state.get(&state_path).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default();
        let mut changed = 0;
        for (path, bytes) in self.engine.sandbox.read_tree(session, OUTPUTS).await.map_err(err)? {
            let digest = hex::encode(Sha256::digest(&bytes));
            let previous = synced.files.get(&path);
            if previous.is_some_and(|(d, _)| *d == digest) {
                continue;
            }
            let name = path.rsplit('/').next().unwrap_or(&path).to_owned();
            let prior = previous.map(|(_, id)| id.clone());
            let id =
                self.engine.assets.save_output(&self.ctx, session, &name, bytes, prior.as_deref()).await?;
            synced.files.insert(path, (digest, id));
            changed += 1;
        }
        if changed > 0 {
            let bytes = serde_json::to_vec(&synced).map_err(err)?;
            self.engine
                .sandbox
                .put_files(session, vec![SandboxFile { path: state_path, bytes, mode: 0o600 }])
                .await
                .map_err(err)?;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_paths_root_under_the_uploads_directory() {
        assert_eq!(
            upload_path("file_1", "/extraction/docs/a.json"),
            "/mnt/session/uploads/extraction/docs/a.json"
        );
        assert_eq!(upload_path("file_1", "/workspace/file_1"), "/mnt/session/uploads/file_1");
    }
}
