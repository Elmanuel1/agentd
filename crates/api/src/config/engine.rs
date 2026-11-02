//! The agent loop's settings: model providers, limits and web search.

use engine::model::ProviderConfig;

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
}

impl std::fmt::Debug for EngineConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineConfig")
            .field("providers", &self.providers)
            .field("extra_models", &self.extra_models)
            .field("max_tokens", &self.max_tokens)
            .field("web_search", &self.search_key.is_some())
            .field("max_active_sessions", &self.max_active_sessions)
            .finish()
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
        let extra_models = get("AGENTD_MODELS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Self {
            providers,
            extra_models,
            max_tokens: number(get, "AGENTD_MAX_OUTPUT_TOKENS", 16000)?,
            search_key: key("AGENTD_BRAVE_API_KEY"),
            max_active_sessions: number(get, "AGENTD_MAX_ACTIVE_SESSIONS", 64)?,
        })
    }
}
