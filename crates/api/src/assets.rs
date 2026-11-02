//! What the agent loop reads from other resources, over their services: file
//! and skill contents, environment configs, and session outputs as files.

use std::sync::Arc;

use async_trait::async_trait;
use engine::worker::setup::{CA_BUNDLE, CA_PATH};
use engine::worker::{SandboxEgress, SessionAssets, SkillFiles};

use crate::egress::SessionEgressPolicy;
use environments::EnvironmentsService;
use files::FilesService;
use shared::{ApiError, AuthContext};
use skills::SkillsService;

/// [`SessionAssets`] over the files, skills and environments services.
#[derive(Clone)]
pub struct RealSessionAssets {
    /// Files and outputs.
    pub files: Arc<FilesService>,
    /// Skill bundles.
    pub skills: Arc<SkillsService>,
    /// Environment configs.
    pub environments: Arc<EnvironmentsService>,
    /// Registers sessions with the egress service; `None` when there is none.
    pub egress: Option<Arc<SessionEgressPolicy>>,
    /// The proxy as a sandbox reaches it (`host:port`).
    pub egress_host: String,
}

impl std::fmt::Debug for RealSessionAssets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealSessionAssets").finish_non_exhaustive()
    }
}

#[async_trait]
impl SessionAssets for RealSessionAssets {
    async fn file(&self, ctx: &AuthContext, file_id: &str) -> Result<Vec<u8>, ApiError> {
        let (_, bytes) = self.files.content(ctx, file_id).await?;
        Ok(bytes.to_vec())
    }

    async fn skill(&self, ctx: &AuthContext, skill_id: &str, version: &str) -> Result<SkillFiles, ApiError> {
        let version = match version {
            "latest" => self.skills.get(ctx, skill_id).await?.latest_version,
            v => v.to_owned(),
        };
        let archive = self.skills.version_bundle(ctx, skill_id, &version).await?;
        let unpacked = skills::unpack(&archive)?;
        Ok(SkillFiles { name: unpacked.directory, files: unpacked.files })
    }

    async fn environment(
        &self,
        ctx: &AuthContext,
        environment_id: &str,
    ) -> Result<serde_json::Value, ApiError> {
        let env = self.environments.get(ctx, environment_id).await?;
        serde_json::to_value(&env.config).map_err(|e| ApiError::internal(e.to_string()))
    }

    async fn egress(&self, ctx: &AuthContext, session: &str) -> Result<SandboxEgress, ApiError> {
        // With no egress service, the sandbox reaches nothing beyond agentd.
        let Some(policy) = &self.egress else { return Ok(SandboxEgress::default()) };
        let prepared = policy.prepare(ctx, session).await?;
        let mut env = prepared.env;
        let proxy = format!("http://{session}:{}@{}", prepared.password, self.egress_host);
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY"] {
            env.insert(name.to_owned(), proxy.clone());
        }
        env.insert("NO_PROXY".into(), "localhost,127.0.0.1".into());
        env.insert("no_proxy".into(), "localhost,127.0.0.1".into());
        for name in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"] {
            env.insert(name.to_owned(), CA_BUNDLE.to_owned());
        }
        env.insert("NODE_EXTRA_CA_CERTS".into(), CA_PATH.to_owned());
        let credentials = prepared.disclosed.into_iter().map(|d| (d.name, d.description, d.hosts)).collect();
        Ok(SandboxEgress { env, ca_pem: Some(prepared.ca_cert), credentials })
    }

    async fn save_output(
        &self,
        ctx: &AuthContext,
        session: &str,
        filename: &str,
        bytes: Vec<u8>,
        previous: Option<&str>,
    ) -> Result<String, ApiError> {
        let meta = self.files.save_output(ctx, session, filename, bytes.into()).await?;
        if let Some(old) = previous {
            if let Err(e) = self.files.delete(ctx, old).await {
                tracing::warn!(file = old, error = %e.message, "engine: could not remove an output's earlier file");
            }
        }
        Ok(meta.id.as_str().to_owned())
    }
}
