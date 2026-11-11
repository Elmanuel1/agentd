//! What a replica runs (`AGENTD_ROLE`), and the loop's settings.

use std::time::Duration;

use sessions::steps::LoopWorkspaces;
use shared::{Timestamp, WorkspaceId};

use super::{number, ConfigError};

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
    /// This replica serves the API's background jobs and launches sandboxes.
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
    /// `AGENTD_LOOP_WORKSPACES`: `wrkspc_…@<RFC 3339>,…`, the workspaces whose
    /// sessions the loop serves and when each joined.
    pub workspaces: LoopWorkspaces,
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
    /// A step's model deadline (10 min); its lease is 2 min longer.
    pub deadline: Duration,
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
                })
            }
        };
        let optional = |var| get(var).map(|_| number(get, var, 1_u64)).transpose();
        Ok(Self {
            role,
            workspaces: workspaces(get("AGENTD_LOOP_WORKSPACES").as_deref().unwrap_or_default())?,
            per_workspace: number(get, "AGENTD_MODEL_CONCURRENCY_PER_WORKSPACE", 32)?,
            attempts: number(get, "AGENTD_MODEL_ATTEMPTS", 5)?,
            steps: number(get, "AGENTD_STEPS_PER_WORKER", 256)?,
            max_output_tokens: number(get, "AGENTD_MAX_OUTPUT_TOKENS", 16000)?,
            rpm: optional("AGENTD_MODEL_RPM")?,
            itpm: optional("AGENTD_MODEL_ITPM")?,
            anthropic_base_url: get("AGENTD_ANTHROPIC_BASE_URL")
                .unwrap_or_else(|| "https://api.anthropic.com".into()),
            deadline: Duration::from_secs(600),
        })
    }
}

fn workspaces(raw: &str) -> Result<LoopWorkspaces, ConfigError> {
    let joined = raw.split(',').map(str::trim).filter(|w| !w.is_empty()).map(|entry| {
        let bad = || ConfigError {
            var: "AGENTD_LOOP_WORKSPACES",
            reason: format!("`{entry}` is not `<workspace>@<RFC 3339 time>`"),
        };
        let (ws, at) = entry.split_once('@').ok_or_else(bad)?;
        let at: Timestamp =
            serde_json::from_value(serde_json::Value::String(at.to_owned())).map_err(|_| bad())?;
        Ok((WorkspaceId::new(ws), at))
    });
    Ok(LoopWorkspaces::new(joined.collect::<Result<Vec<_>, ConfigError>>()?))
}
