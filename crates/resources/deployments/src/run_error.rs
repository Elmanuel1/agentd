//! What a failed fire records: the run's error, and what it does to the
//! deployment.

use serde::{Deserialize, Serialize};
use sessions::service::FireRefusal;
use shared::Timestamp;

use crate::types::{Deployment, DeploymentStatus};

/// Why a deployment is paused. Live: `{"type":"manual"}`, or
/// `{"type":"error","error":{"type":"mcp_egress_blocked_error",...}}` (B22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PausedReason {
    /// Paused by the API.
    Manual,
    /// Paused by a fire that cannot succeed until the deployment changes.
    Error {
        /// The fire's error.
        error: RunError,
    },
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
    /// An MCP server host of the agent is blocked by the environment's
    /// networking; the deployment pauses.
    #[serde(rename = "mcp_egress_blocked_error")]
    McpEgressBlocked,
}

/// Map a session-create rejection into the fixed run-error taxonomy.
pub(crate) fn classify_fire_error(refusal: &FireRefusal) -> RunError {
    let e = match refusal {
        FireRefusal::EgressBlocked => {
            let message = sessions::service::MCP_EGRESS_BLOCKED.to_owned();
            return RunError { kind: RunErrorKind::McpEgressBlocked, message };
        }
        FireRefusal::Api(e) => e,
    };
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

/// What a failed fire does to its deployment's schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Nothing: the next occurrence tries again.
    Keep,
    /// The deployment paused.
    Pause,
    /// The deployment archived.
    Archive,
}

/// Apply a failed fire to its deployment.
#[must_use]
pub(crate) fn apply_run_error(d: &mut Deployment, error: &RunError) -> Applied {
    match error.kind {
        // Documented: agent archived/deleted → deployment auto-archives.
        RunErrorKind::AgentArchived => {
            d.status = DeploymentStatus::Archived;
            d.archived_at = Some(Timestamp::now());
            Applied::Archive
        }
        RunErrorKind::McpEgressBlocked => {
            d.status = DeploymentStatus::Paused;
            d.paused_reason = Some(PausedReason::Error { error: error.clone() });
            Applied::Pause
        }
        _ => Applied::Keep,
    }
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
            (RunErrorKind::McpEgressBlocked, "mcp_egress_blocked_error"),
        ] {
            assert_eq!(serde_json::to_value(k).unwrap(), s);
        }
    }

    fn deployment() -> Deployment {
        serde_json::from_value(serde_json::json!({
            "agent": {"id": "agent_011CZkYnKw7TfQv5X2mY8wP4", "type": "agent", "version": 1},
            "archived_at": null,
            "created_at": "2026-07-03T22:41:34.000000Z",
            "description": null,
            "environment_id": "env_011CZkYnKw7TfQv5X2mY8wP4",
            "id": "depl_011CZkYnKw7TfQv5X2mY8wP4",
            "initial_events": [],
            "metadata": {},
            "name": "d",
            "paused_reason": null,
            "resources": [],
            "schedule": {
                "expression": "0 0 1 1 *",
                "last_run_at": null,
                "timezone": "UTC",
                "type": "cron",
                "upcoming_runs_at": []
            },
            "status": "active",
            "type": "deployment",
            "updated_at": "2026-07-03T22:41:34.000000Z",
            "vault_ids": []
        }))
        .unwrap()
    }

    fn error(kind: RunErrorKind) -> RunError {
        RunError { kind, message: "m".into() }
    }

    #[test]
    fn an_archived_agent_archives_the_deployment() {
        let mut d = deployment();
        assert_eq!(apply_run_error(&mut d, &error(RunErrorKind::AgentArchived)), Applied::Archive);
        assert!(d.archived_at.is_some());
        let mut want = deployment();
        want.status = DeploymentStatus::Archived;
        want.archived_at = d.archived_at;
        assert_eq!(d, want);
    }

    #[test]
    fn any_other_run_error_leaves_the_deployment_as_it_was() {
        for kind in [
            RunErrorKind::EnvironmentArchived,
            RunErrorKind::VaultNotFound,
            RunErrorKind::SessionRateLimited,
            RunErrorKind::ServiceUnavailable,
        ] {
            let mut d = deployment();
            assert_eq!(apply_run_error(&mut d, &error(kind)), Applied::Keep);
            assert_eq!(d, deployment());
        }
    }
}
