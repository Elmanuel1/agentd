//! How workers reach agentd, and the key rings its work tokens and
//! placeholders are signed with.

use shared::hash::KeyRing;

use super::ConfigError;

/// The work secret's side of agentd.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkConfig {
    /// `AGENTD_API_BASE_URL` (required): agentd's API as a worker reaches it,
    /// the work secret's `api_base_url`.
    pub api_base_url: String,
    /// `AGENTD_WORK_TOKEN_KEYS` (required): `<id>:<hex>,…`, the first signs
    /// work tokens and every one verifies them.
    pub work_tokens: KeyRing,
    /// `AGENTD_PLACEHOLDER_KEYS` (required): the same, for the placeholders of
    /// `environment_variable` credentials in a `cloud` work secret.
    pub placeholders: KeyRing,
}

impl std::fmt::Debug for WorkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkConfig")
            .field("api_base_url", &self.api_base_url)
            .field("work_tokens", &self.work_tokens)
            .field("placeholders", &self.placeholders)
            .finish()
    }
}

impl WorkConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let required = |var: &'static str, what: &str| {
            get(var)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| ConfigError { var, reason: format!("required: {what}") })
        };
        let ring = |var: &'static str| {
            KeyRing::parse(&required(var, "a key ring `<id>:<hex>,…`")?)
                .map_err(|e| ConfigError { var, reason: e.to_string() })
        };
        Ok(Self {
            api_base_url: required("AGENTD_API_BASE_URL", "agentd's API as workers reach it")?,
            work_tokens: ring("AGENTD_WORK_TOKEN_KEYS")?,
            placeholders: ring("AGENTD_PLACEHOLDER_KEYS")?,
        })
    }
}
