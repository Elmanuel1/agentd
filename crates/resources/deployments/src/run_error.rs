//! What a failed fire records: the run's error, and what it does to the
//! deployment.

use serde::{Deserialize, Serialize};
use shared::{ApiError, Timestamp};

use crate::types::{Deployment, DeploymentStatus};

/// Why a deployment is paused. Live: `{"type":"manual"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PausedReason {
    /// Paused by the API.
    Manual,
}

/// Fire error taxonomy (documented).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunError {
    /// One of: environment_archived · agent_archived · vault_not_found ·
    /// session_rate_limited · service_unavailable.
    #[serde(rename = "type")]
    pub kind: RunErrorKind,
    /// Human detail.
    pub message: String,
}

/// The fixed error kinds a run can record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunErrorKind {
    /// Environment archived — fix or archive the deployment.
    EnvironmentArchived,
    /// Agent archived — the deployment auto-archives.
    AgentArchived,
    /// Vault missing.
    VaultNotFound,
    /// Rate limited — no retry; next occurrence tries again.
    SessionRateLimited,
    /// Transient service failure.
    ServiceUnavailable,
}

/// Map a session-create rejection into the fixed run-error taxonomy.
pub(crate) fn classify_fire_error(e: &ApiError) -> RunError {
    let msg = e.message.clone();
    let kind = if msg.contains("agent") && msg.contains("archived") {
        RunErrorKind::AgentArchived
    } else if msg.contains("environment") && msg.contains("archived") {
        RunErrorKind::EnvironmentArchived
    } else if msg.contains("vault") {
        RunErrorKind::VaultNotFound
    } else if e.status == 429 {
        RunErrorKind::SessionRateLimited
    } else {
        RunErrorKind::ServiceUnavailable
    };
    RunError { kind, message: msg }
}

/// Apply a failed fire to its deployment; true when the deployment archived,
/// so the caller deletes its timer.
pub(crate) fn apply_run_error(d: &mut Deployment, error: &RunError) -> bool {
    // Documented: agent archived/deleted → deployment auto-archives.
    if error.kind != RunErrorKind::AgentArchived {
        return false;
    }
    d.status = DeploymentStatus::Archived;
    d.archived_at = Some(Timestamp::now());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_error_kinds_serialize_to_documented_strings() {
        for (k, s) in [
            (RunErrorKind::EnvironmentArchived, "environment_archived"),
            (RunErrorKind::AgentArchived, "agent_archived"),
            (RunErrorKind::VaultNotFound, "vault_not_found"),
            (RunErrorKind::SessionRateLimited, "session_rate_limited"),
            (RunErrorKind::ServiceUnavailable, "service_unavailable"),
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), s);
        }
    }
}
