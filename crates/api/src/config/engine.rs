//! The agent loop's settings: model providers, limits and web search.

use engine::model::ProviderConfig;

use super::runtime::{port, sandbox_reach};
use super::{number, ConfigError};

/// The agent loop. It runs whenever the sandbox runtime is `docker`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EngineConfig {
    /// `AGENTD_ANTHROPIC_API_KEY` (+ `AGENTD_ANTHROPIC_BASE_URL`) for
    /// `claude-*` models; `AGENTD_OPENAI_API_KEY` (+ `AGENTD_OPENAI_BASE_URL`,
    /// OpenRouter by default) for `vendor/model` ids; `AGENTD_MOCK_MODEL`, a
    /// per-agent script that answers every request instead.
    pub providers: ProviderConfig,
    /// `AGENTD_MODELS`: model ids agents may name beyond the built-in
    /// Anthropic ones (comma-separated).
    pub extra_models: Vec<String>,
    /// `AGENTD_MAX_OUTPUT_TOKENS` (default 16000) per model call.
    pub max_tokens: u32,
    /// `AGENTD_BRAVE_API_KEY`, for `web_search`.
    pub search_key: Option<String>,
    /// `AGENTD_MAX_ACTIVE_SESSIONS` (default 64): session loops this replica
    /// runs at once, each holding a lease connection.
    pub max_active_sessions: usize,
    /// `AGENTD_SANDBOX_API_URL`: agentd's API as a sandbox's worker reaches
    /// it (default `http://<agentd-gateway or AGENTD_SANDBOX_JOIN>:<AGENTD_ADDR port>`).
    pub sandbox_api_url: String,
    /// `AGENTD_WORK_TOKEN_KEY` (hex): signs work tokens and egress proxy
    /// passwords; random per process when unset, which suits one replica.
    pub work_token_key: Option<Vec<u8>>,
    /// `AGENTD_EGRESS_ADDR` (default `0.0.0.0:8890`): where the egress proxy
    /// listens.
    pub egress_addr: String,
    /// `AGENTD_SANDBOX_EGRESS_HOST`: the proxy as a sandbox reaches it
    /// (default `<agentd-gateway or AGENTD_SANDBOX_JOIN>:<AGENTD_EGRESS_ADDR port>`).
    pub sandbox_egress_host: String,
    /// `AGENTD_EGRESS_INTERNAL_HOSTS`: hosts sandboxes may reach at internal
    /// addresses (comma-separated); none by default.
    pub egress_internal_hosts: Vec<String>,
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("providers", &self.providers)
            .field("extra_models", &self.extra_models)
            .field("max_tokens", &self.max_tokens)
            .field("web_search", &self.search_key.is_some())
            .field("max_active_sessions", &self.max_active_sessions)
            .field("sandbox_api_url", &self.sandbox_api_url)
            .finish_non_exhaustive()
    }
}

impl EngineConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let key = |var: &str| get(var).filter(|v| !v.is_empty());
        let providers = ProviderConfig {
            anthropic: key("AGENTD_ANTHROPIC_API_KEY").map(|k| {
                (k, get("AGENTD_ANTHROPIC_BASE_URL").unwrap_or_else(|| "https://api.anthropic.com".into()))
            }),
            openai_compat: key("AGENTD_OPENAI_API_KEY").map(|k| {
                (k, get("AGENTD_OPENAI_BASE_URL").unwrap_or_else(|| "https://openrouter.ai/api/v1".into()))
            }),
            mock_script: key("AGENTD_MOCK_MODEL").map(Into::into),
        };
        let extra_models = list(get("AGENTD_MODELS"));
        Ok(Self {
            providers,
            extra_models,
            max_tokens: number(get, "AGENTD_MAX_OUTPUT_TOKENS", 16000)?,
            search_key: key("AGENTD_BRAVE_API_KEY"),
            max_active_sessions: number(get, "AGENTD_MAX_ACTIVE_SESSIONS", 64)?,
            sandbox_api_url: get("AGENTD_SANDBOX_API_URL").unwrap_or_else(|| {
                format!("http://{}:{}", sandbox_reach(get), port(get("AGENTD_ADDR"), 8080))
            }),
            egress_addr: get("AGENTD_EGRESS_ADDR").unwrap_or_else(|| "0.0.0.0:8890".into()),
            sandbox_egress_host: get("AGENTD_SANDBOX_EGRESS_HOST").unwrap_or_else(|| {
                let addr = get("AGENTD_EGRESS_ADDR").unwrap_or_default();
                format!(
                    "host.docker.internal:{}",
                    addr.rsplit(':').next().filter(|p| !p.is_empty()).unwrap_or("8890")
                )
            }),
            egress_internal_hosts: list(get("AGENTD_EGRESS_INTERNAL_HOSTS")),
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

/// A comma-separated list.
fn list(raw: Option<String>) -> Vec<String> {
    raw.unwrap_or_default().split(',').map(str::trim).filter(|m| !m.is_empty()).map(str::to_owned).collect()
}
