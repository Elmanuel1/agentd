//! How each session's sandbox reaches agentd and the egress service.

use super::runtime::{port, sandbox_reach};
use super::ConfigError;

/// The sandbox's side of agentd: where its harness reports and how its
/// traffic leaves.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SandboxConfig {
    /// `AGENTD_SANDBOX_API_URL`: agentd's API as a sandbox's harness reaches
    /// it (default `http://<agentd-gateway or the first AGENTD_SANDBOX_JOIN container>:<AGENTD_ADDR port>`).
    pub api_url: String,
    /// `AGENTD_WORK_TOKEN_KEY` (hex): signs work tokens and egress proxy
    /// passwords; random per process when unset, which suits one replica.
    pub work_token_key: Option<Vec<u8>>,
    /// `AGENTD_EGRESS_ADMIN_URL` and `AGENTD_EGRESS_ADMIN_TOKEN`: the egress
    /// service's admin interface and the token it takes. Unset, sandboxes
    /// reach nothing beyond agentd.
    pub egress_admin: Option<(String, String)>,
    /// `AGENTD_SANDBOX_EGRESS_HOST`: the egress proxy as a sandbox reaches it
    /// (default `<agentd-gateway or the second AGENTD_SANDBOX_JOIN container>:<AGENTD_EGRESS_ADDR port>`).
    pub egress_host: String,
    /// `AGENTD_MODEL_BASE_URL` (default `https://openrouter.ai/api/v1`): the
    /// OpenAI-compatible API a sandbox's harness calls the model at, through
    /// the egress proxy.
    pub model_base_url: String,
}

impl std::fmt::Debug for SandboxConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxConfig")
            .field("api_url", &self.api_url)
            .field("egress_admin_url", &self.egress_admin.as_ref().map(|(url, _)| url))
            .field("egress_host", &self.egress_host)
            .field("model_base_url", &self.model_base_url)
            .finish_non_exhaustive()
    }
}

impl SandboxConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let key = |var: &str| get(var).filter(|v| !v.is_empty());
        Ok(Self {
            api_url: get("AGENTD_SANDBOX_API_URL").unwrap_or_else(|| {
                format!("http://{}:{}", sandbox_reach(get, 0), port(get("AGENTD_ADDR"), 8080))
            }),
            egress_admin: match (key("AGENTD_EGRESS_ADMIN_URL"), key("AGENTD_EGRESS_ADMIN_TOKEN")) {
                (Some(url), Some(token)) => Some((url, token)),
                (None, None) => None,
                _ => {
                    return Err(ConfigError {
                        var: "AGENTD_EGRESS_ADMIN_URL",
                        reason: "set with AGENTD_EGRESS_ADMIN_TOKEN, or neither".into(),
                    })
                }
            },
            egress_host: get("AGENTD_SANDBOX_EGRESS_HOST").unwrap_or_else(|| {
                format!("{}:{}", sandbox_reach(get, 1), port(get("AGENTD_EGRESS_ADDR"), 8890))
            }),
            model_base_url: get("AGENTD_MODEL_BASE_URL")
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".into()),
            work_token_key: match key("AGENTD_WORK_TOKEN_KEY") {
                Some(hex) => Some(decode_hex(&hex).ok_or_else(|| ConfigError {
                    var: "AGENTD_WORK_TOKEN_KEY",
                    reason: "must be at least 16 bytes of hex".into(),
                })?),
                None => None,
            },
        })
    }
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0 && s.len() >= 32)
        .then(|| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect())?
}
