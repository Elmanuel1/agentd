//! How workers reach agentd, and the key their work tokens are signed with.

use super::ConfigError;

/// The work secret's side of agentd.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkConfig {
    /// `AGENTD_API_BASE_URL` (required): agentd's API as a worker reaches it,
    /// the work secret's `api_base_url`.
    pub api_base_url: String,
    /// `AGENTD_WORK_TOKEN_KEY` (hex): signs work tokens in place of
    /// `<prefix>/system/work_token_key`.
    pub work_token_key: Option<Vec<u8>>,
}

impl std::fmt::Debug for WorkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkConfig").field("api_base_url", &self.api_base_url).finish_non_exhaustive()
    }
}

impl WorkConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let key = |var: &str| get(var).filter(|v| !v.is_empty());
        Ok(Self {
            api_base_url: key("AGENTD_API_BASE_URL").ok_or_else(|| ConfigError {
                var: "AGENTD_API_BASE_URL",
                reason: "required: agentd's API as workers reach it".into(),
            })?,
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
