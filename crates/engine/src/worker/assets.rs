//! What the worker needs from other resources: file and skill contents, the
//! environment's config, and a place to keep the session's outputs.

use async_trait::async_trait;
use shared::{ApiError, AuthContext};

/// A skill version's files, relative to the skill's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFiles {
    /// The skill's directory name.
    pub name: String,
    /// `(relative path, bytes)`.
    pub files: Vec<(String, Vec<u8>)>,
}

/// What a session's sandbox is told about egress: the environment its
/// commands run with (each credential's placeholder, the proxy, the CA
/// bundle), the session CA, and the credentials as the agent may know them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxEgress {
    /// Variables for every command.
    pub env: std::collections::BTreeMap<String, String>,
    /// The session CA the sandbox trusts, PEM.
    pub ca_pem: Option<String>,
    /// `(name, description, hosts)` of each credential; never a value.
    pub credentials: Vec<(String, String, Option<Vec<String>>)>,
}

/// Files, skills, environments and session outputs, by id, as `ctx` (the
/// caller whose events started the loop) sees them.
#[async_trait]
pub trait SessionAssets: Send + Sync {
    /// A file's bytes.
    async fn file(&self, ctx: &AuthContext, file_id: &str) -> Result<Vec<u8>, ApiError>;
    /// A skill version's files (`version` is a number or `latest`).
    async fn skill(&self, ctx: &AuthContext, skill_id: &str, version: &str) -> Result<SkillFiles, ApiError>;
    /// An environment's `config`.
    async fn environment(
        &self,
        ctx: &AuthContext,
        environment_id: &str,
    ) -> Result<serde_json::Value, ApiError>;
    /// The egress setup of `session`'s sandbox.
    async fn egress(&self, ctx: &AuthContext, session: &str) -> Result<SandboxEgress, ApiError>;
    /// Keep `bytes` as a file scoped to `session` named `filename`, in place
    /// of `previous` (an earlier version of the same output). The new file id.
    async fn save_output(
        &self,
        ctx: &AuthContext,
        session: &str,
        filename: &str,
        bytes: Vec<u8>,
        previous: Option<&str>,
    ) -> Result<String, ApiError>;
}
