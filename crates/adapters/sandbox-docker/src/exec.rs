//! The sandbox port on Docker: commands through `docker exec` as the
//! sandbox user, files through the container archive API.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

use async_trait::async_trait;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::query_parameters::{DownloadFromContainerOptionsBuilder, UploadToContainerOptionsBuilder};
use futures::{StreamExt, TryStreamExt};
use shared::ports::sandbox::{ExecOutput, ExecRequest, Owner, Sandbox, SandboxFile};
use shared::ports::RuntimeError;
use tokio::io::AsyncWriteExt;

use super::{not_found, DockerRuntime};

/// The sandbox user's uid and gid.
const AGENT_ID: u64 = 65534;

/// Most bytes of stdout plus stderr kept from one command.
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// Exit status `timeout -s KILL` gives a command it killed.
const KILLED: i64 = 137;

fn unavailable(e: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Unavailable(e.to_string())
}

/// A tar archive of `entries` (`(absolute path, bytes, mode, uid)`; `None`
/// bytes make a directory), to extract at `/`.
fn archive(entries: &[(&str, Option<&[u8]>, u32, u64)]) -> Result<Vec<u8>, RuntimeError> {
    let mut tar = tar::Builder::new(Vec::new());
    for (path, bytes, mode, uid) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(*mode);
        header.set_uid(*uid);
        header.set_gid(*uid);
        header.set_mtime(0);
        let rel = path.trim_start_matches('/');
        match bytes {
            Some(b) => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(b.len() as u64);
                tar.append_data(&mut header, rel, *b).map_err(unavailable)?;
            }
            None => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                tar.append_data(&mut header, rel, std::io::empty()).map_err(unavailable)?;
            }
        }
    }
    tar.into_inner().map_err(unavailable)
}

impl DockerRuntime {
    async fn upload(&self, target_id: &str, tar: Vec<u8>) -> Result<(), RuntimeError> {
        let name = Self::container_name(target_id);
        let options = UploadToContainerOptionsBuilder::default().path("/").build();
        match self.docker.upload_to_container(&name, Some(options), bollard::body_full(tar.into())).await {
            Ok(()) => Ok(()),
            Err(e) if not_found(&e) => Err(RuntimeError::Unknown(name)),
            Err(e) => Err(unavailable(e)),
        }
    }
}

#[async_trait]
impl Sandbox for DockerRuntime {
    async fn exec(&self, target_id: &str, req: ExecRequest) -> Result<ExecOutput, RuntimeError> {
        let name = Self::container_name(target_id);
        let secs = req.timeout.as_secs().max(1).to_string();
        let mut cmd = vec!["timeout".to_owned(), "-s".to_owned(), "KILL".to_owned(), secs];
        cmd.extend(req.argv);
        let options = CreateExecOptions {
            cmd: Some(cmd),
            attach_stdin: Some(req.stdin.is_some()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            working_dir: req.workdir,
            ..Default::default()
        };
        let created = match self.docker.create_exec(&name, options).await {
            Ok(c) => c,
            Err(e) if not_found(&e) => return Err(RuntimeError::Unknown(name)),
            Err(e) => return Err(unavailable(e)),
        };
        let started = self
            .docker
            .start_exec(&created.id, Some(StartExecOptions::default()))
            .await
            .map_err(unavailable)?;
        let StartExecResults::Attached { mut output, mut input } = started else {
            return Err(unavailable("exec started detached"));
        };
        if let Some(bytes) = req.stdin {
            input.write_all(&bytes).await.map_err(unavailable)?;
            input.shutdown().await.map_err(unavailable)?;
        }
        drop(input);
        let mut out = ExecOutput::default();
        let read = async {
            while let Some(chunk) = output.next().await {
                let chunk = chunk.map_err(unavailable)?;
                let (sink, bytes) = match chunk {
                    bollard::container::LogOutput::StdErr { message } => (&mut out.stderr, message),
                    other => (&mut out.stdout, other.into_bytes()),
                };
                if sink.len() < MAX_OUTPUT {
                    sink.extend_from_slice(&bytes);
                }
            }
            Ok::<_, RuntimeError>(())
        };
        // The in-container `timeout` kills the command; this bounds a stream
        // the daemon never closes.
        if tokio::time::timeout(req.timeout + Duration::from_secs(30), read).await.is_err() {
            out.timed_out = true;
        }
        let inspected = self.docker.inspect_exec(&created.id).await.map_err(unavailable)?;
        out.exit_code = inspected.exit_code.unwrap_or(-1);
        out.timed_out |= out.exit_code == KILLED;
        Ok(out)
    }

    async fn put_files(&self, target_id: &str, files: Vec<SandboxFile>) -> Result<(), RuntimeError> {
        if files.is_empty() {
            return Ok(());
        }
        let entries: Vec<(&str, Option<&[u8]>, u32, u64)> =
            files.iter().map(|f| (f.path.as_str(), Some(f.bytes.as_slice()), f.mode, 0)).collect();
        self.upload(target_id, archive(&entries)?).await
    }

    async fn make_dir(&self, target_id: &str, dir: &str, owner: Owner) -> Result<(), RuntimeError> {
        let uid = match owner {
            Owner::Root => 0,
            Owner::Agent => AGENT_ID,
        };
        self.upload(target_id, archive(&[(dir, None, 0o755, uid)])?).await
    }

    async fn read_tree(&self, target_id: &str, dir: &str) -> Result<BTreeMap<String, Vec<u8>>, RuntimeError> {
        let name = Self::container_name(target_id);
        let options = DownloadFromContainerOptionsBuilder::default().path(dir).build();
        let chunks: Vec<bytes::Bytes> =
            match self.docker.download_from_container(&name, Some(options)).try_collect().await {
                Ok(c) => c,
                Err(e) if not_found(&e) => return Ok(BTreeMap::new()),
                Err(e) => return Err(unavailable(e)),
            };
        let tar: Vec<u8> = chunks.concat();
        // The archive names entries from the directory's own name down.
        let parent = dir.trim_end_matches('/').rsplit_once('/').map_or("", |(p, _)| p);
        let mut files = BTreeMap::new();
        let mut archive = tar::Archive::new(tar.as_slice());
        for entry in archive.entries().map_err(unavailable)? {
            let mut entry = entry.map_err(unavailable)?;
            if entry.header().entry_type() != tar::EntryType::Regular {
                continue;
            }
            let rel = entry.path().map_err(unavailable)?.to_string_lossy().into_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(unavailable)?;
            files.insert(format!("{parent}/{rel}"), bytes);
        }
        Ok(files)
    }
}
