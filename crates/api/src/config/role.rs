//! What a replica runs (`AGENTD_ROLE`), and the loop's settings.

use std::time::Duration;

use super::{ConfigError, number};

/// `AGENTD_ROLE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Role {
    /// The API, with no loop worker.
    Api,
    /// Loop workers only.
    Loop,
    /// Both.
    #[default]
    All,
}

impl Role {
    /// This replica serves the API and its background jobs.
    pub fn serves_api(self) -> bool {
        self != Self::Loop
    }

    /// This replica runs loop workers.
    pub fn runs_loop(self) -> bool {
        self != Self::Api
    }
}

/// The loop's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopConfig {
    /// What this replica runs.
    pub role: Role,
    /// `AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE` (32).
    pub per_workspace: i64,
    /// `AGENTD_MODEL_ATTEMPTS` (5).
    pub attempts: u32,
    /// `AGENTD_STEPS_PER_WORKER` (256).
    pub steps: usize,
    /// `AGENTD_MAX_OUTPUT_TOKENS` (16000).
    pub max_output_tokens: u32,
    /// `AGENTD_MODEL_RPM`: requests per provider and minute; unset, no limit.
    pub rpm: Option<u64>,
    /// `AGENTD_MODEL_ITPM`: input tokens per provider and minute; unset, no limit.
    pub itpm: Option<u64>,
    /// `AGENTD_ANTHROPIC_BASE_URL` (`https://api.anthropic.com`).
    pub anthropic_base_url: String,
    /// `AGENTD_MODEL_BASE_URL` (`https://openrouter.ai/api/v1`): the
    /// OpenAI-compatible API other models are called at.
    pub model_base_url: String,
    /// A step's model deadline (10 min); its lease is 2 min longer.
    pub deadline: Duration,
    /// `AGENTD_COMPACTION_CLEAR_AT` (100000), `AGENTD_COMPACTION_KEEP_RESULTS`
    /// (3, from 3 to 5) and `AGENTD_TOOL_OUTPUT_TOKEN_LIMIT` (16000).
    pub compaction: engine::turn::compact::Compaction,
}

impl LoopConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let role = match get("AGENTD_ROLE").as_deref() {
            None | Some("all") => Role::All,
            Some("api") => Role::Api,
            Some("loop") => Role::Loop,
            Some(other) => {
                return Err(ConfigError {
                    var: "AGENTD_ROLE",
                    reason: format!("expected api, loop or all, got `{other}`"),
                });
            }
        };
        let optional = |var| get(var).map(|_| number(get, var, 1_u64)).transpose();
        Ok(Self {
            role,
            per_workspace: number(get, "AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE", 32)?,
            attempts: number(get, "AGENTD_MODEL_ATTEMPTS", 5)?,
            steps: number(get, "AGENTD_STEPS_PER_WORKER", 256)?,
            max_output_tokens: number(get, "AGENTD_MAX_OUTPUT_TOKENS", 16000)?,
            rpm: optional("AGENTD_MODEL_RPM")?,
            itpm: optional("AGENTD_MODEL_ITPM")?,
            anthropic_base_url: get("AGENTD_ANTHROPIC_BASE_URL")
                .unwrap_or_else(|| "https://api.anthropic.com".into()),
            model_base_url: get("AGENTD_MODEL_BASE_URL")
                .unwrap_or_else(|| "https://openrouter.ai/api/v1".into()),
            deadline: Duration::from_secs(600),
            compaction: compaction(get)?,
        })
    }
}

fn compaction(
    get: &impl Fn(&str) -> Option<String>,
) -> Result<engine::turn::compact::Compaction, ConfigError> {
    let keep_results: usize = number(get, "AGENTD_COMPACTION_KEEP_RESULTS", 3)?;
    if !(3..=5).contains(&keep_results) {
        return Err(ConfigError { var: "AGENTD_COMPACTION_KEEP_RESULTS", reason: "expected 3 to 5".into() });
    }
    Ok(engine::turn::compact::Compaction {
        clear_at: number(get, "AGENTD_COMPACTION_CLEAR_AT", 100_000)?,
        keep_results,
        tool_output_limit: number(get, "AGENTD_TOOL_OUTPUT_TOKEN_LIMIT", 16_000)?,
    })
}
