//! Deployment wire types — shapes from the live fixture.

use agents::AgentRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use shared::schedule::{CompiledSchedule, CronKind, CronSchedule};
use shared::{ApiError, ContentBlock, Id, Metadata, Resource, Timestamp};

use crate::run_error::{PausedReason, RunError};

/// Marker for `depl_…`.
#[derive(Debug, Clone, Copy)]
pub struct DeploymentResource_;
impl Resource for DeploymentResource_ {
    const PREFIX: &'static str = "depl";
    const LABEL: &'static str = "deployment";
}
/// `Id<DeploymentResource_>` alias.
pub type DeploymentId = Id<DeploymentResource_>;

/// Marker for `drun_…`.
#[derive(Debug, Clone, Copy)]
pub struct DeploymentRunResource_;
impl Resource for DeploymentRunResource_ {
    const PREFIX: &'static str = "drun";
    const LABEL: &'static str = "deployment run";
}
/// `Id<DeploymentRunResource_>` alias.
pub type DeploymentRunId = Id<DeploymentRunResource_>;

/// Constant `"type":"deployment"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTag {
    /// Always `deployment`.
    Deployment,
}

/// Constant `"type":"deployment_run"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentRunTag {
    /// Always `deployment_run`.
    DeploymentRun,
}

/// Deployment status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    /// Firing on schedule.
    Active,
    /// Scheduled fires suppressed (manual runs still allowed).
    Paused,
    /// Terminal.
    Archived,
}

/// The wire schedule block — live fixture:
/// `{expression, last_run_at, timezone, type:"cron", upcoming_runs_at:[5]}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireSchedule {
    /// Cron expression.
    pub expression: String,
    /// Last fire (null until first).
    pub last_run_at: Option<Timestamp>,
    /// IANA timezone.
    pub timezone: String,
    /// Always `"cron"`.
    #[serde(rename = "type")]
    pub kind: CronKind,
    /// Next 5 fire instants, UTC.
    pub upcoming_runs_at: Vec<Timestamp>,
}

impl WireSchedule {
    /// Compile through the shared engine (validates cron + tz).
    pub fn compile(&self) -> Result<CompiledSchedule, ApiError> {
        CronSchedule {
            kind: CronKind::Cron,
            expression: self.expression.clone(),
            timezone: self.timezone.clone(),
        }
        .compile()
        .map_err(|e| ApiError::invalid_request(format!("schedule: {e}")))
    }

    /// Recompute `upcoming_runs_at` from `after`.
    pub fn refresh_upcoming(&mut self, after: DateTime<Utc>) -> Result<(), ApiError> {
        let compiled = self.compile()?;
        self.upcoming_runs_at =
            compiled.upcoming_runs_at(after).into_iter().map(Timestamp::from_datetime).collect();
        Ok(())
    }
}

/// What triggered a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerContext {
    /// A scheduled tick.
    Schedule {
        /// The tick's wall-clock instant.
        scheduled_at: Timestamp,
    },
    /// `POST /v1/deployments/{id}/run`.
    Manual {},
}

/// Full wire deployment (field order per fixture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deployment {
    /// Compact agent reference (live: NOT the full snapshot).
    pub agent: AgentRef,
    /// Archive stamp.
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// Description.
    pub description: Option<String>,
    /// Environment.
    pub environment_id: String,
    /// `depl_…`.
    pub id: DeploymentId,
    /// Kickoff events for each fired session.
    pub initial_events: Vec<InitialEvent>,
    /// Metadata.
    pub metadata: Metadata,
    /// Name.
    pub name: String,
    /// Pause reason (null when active).
    pub paused_reason: Option<PausedReason>,
    /// Session resources for each fired session.
    pub resources: Vec<serde_json::Value>,
    /// The schedule.
    pub schedule: WireSchedule,
    /// Status.
    pub status: DeploymentStatus,
    /// Always `"deployment"`.
    #[serde(rename = "type")]
    pub tag: DeploymentTag,
    /// Updated.
    pub updated_at: Timestamp,
    /// Vaults for each fired session.
    pub vault_ids: Vec<String>,
}

/// A kickoff event (`user.message` on the wire).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum InitialEvent {
    /// The starting user message.
    #[serde(rename = "user.message")]
    UserMessage {
        /// Content blocks.
        content: Vec<ContentBlock>,
    },
}

/// One run record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeploymentRun {
    /// Frozen agent ref at fire time.
    pub agent: AgentRef,
    /// Created.
    pub created_at: Timestamp,
    /// Parent deployment.
    pub deployment_id: DeploymentId,
    /// Fire error (null on success).
    pub error: Option<RunError>,
    /// `drun_…`.
    pub id: DeploymentRunId,
    /// The created session (null on failure).
    pub session_id: Option<String>,
    /// What fired.
    pub trigger_context: TriggerContext,
    /// Always `"deployment_run"`.
    #[serde(rename = "type")]
    pub tag: DeploymentRunTag,
}

/// `POST /v1/deployments` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDeploymentRequest {
    /// Required.
    pub name: Option<String>,
    /// Required agent ref (string shorthand or tagged).
    pub agent: Option<sessions::types::AgentRefInput>,
    /// Required.
    pub environment_id: Option<String>,
    /// Required — must contain the starting `user.message`.
    #[serde(default)]
    pub initial_events: Vec<InitialEvent>,
    /// Required.
    pub schedule: Option<CreateScheduleInput>,
    /// Optional.
    pub description: Option<String>,
    /// Optional.
    #[serde(default)]
    pub resources: Vec<serde_json::Value>,
    /// Optional.
    #[serde(default)]
    pub vault_ids: Vec<String>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}

/// Schedule input on create.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateScheduleInput {
    /// Must be `"cron"`.
    #[serde(rename = "type")]
    pub kind: CronKind,
    /// Cron expression.
    pub expression: String,
    /// IANA timezone.
    pub timezone: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_schedule_matches_live_fixture_block() {
        // From the live deployment probe.
        let raw = serde_json::json!({
            "expression": "0 0 1 1 *",
            "last_run_at": null,
            "timezone": "America/New_York",
            "type": "cron",
            "upcoming_runs_at": [
                "2027-01-01T05:00:00.000000Z",
                "2028-01-01T05:00:00.000000Z",
                "2029-01-01T05:00:00.000000Z",
                "2030-01-01T05:00:00.000000Z",
                "2031-01-01T05:00:00.000000Z"
            ]
        });
        let s: WireSchedule = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&s).unwrap(), raw);
    }

    #[test]
    fn refresh_upcoming_reproduces_live_fire_times() {
        let mut s = WireSchedule {
            expression: "0 0 1 1 *".into(),
            last_run_at: None,
            timezone: "America/New_York".into(),
            kind: CronKind::Cron,
            upcoming_runs_at: vec![],
        };
        let after =
            chrono::DateTime::parse_from_rfc3339("2026-07-03T22:41:34Z").unwrap().with_timezone(&chrono::Utc);
        s.refresh_upcoming(after).unwrap();
        assert_eq!(s.upcoming_runs_at.len(), 5);
        assert_eq!(s.upcoming_runs_at[0].to_wire(), "2027-01-01T05:00:00.000000Z");
        assert_eq!(s.upcoming_runs_at[4].to_wire(), "2031-01-01T05:00:00.000000Z");
    }

    #[test]
    fn initial_event_wire_shape() {
        let raw = serde_json::json!({
            "content": [{"text":"run","type":"text"}],
            "type": "user.message"
        });
        let e: InitialEvent = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&e).unwrap(), raw);
    }
}
