//! The scheduler port: durable timers that fire a registered target.
//!
//! Scheduler fire contract (fixed, backend-independent): a sink delivers each
//! occurrence to the fire queue ([`super::fire::FireRegistry::enqueue`]), whose
//! workers run it once per `(target, scheduled_at)` and retry an infra
//! failure; **no backfill**: after downtime only the latest missed occurrence
//! is delivered. `upcoming_runs_at` is computed by [`crate::schedule`], never
//! by the backend, so the wire is identical across backends.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::schedule::{CompiledSchedule, CronKind, CronSchedule, ScheduleError};

// ─── Scheduler port ────────────────────────────────────────────────────────

/// What a schedule fires: the owning resource's [`super::fire::FireHandler::kind`]
/// and the id of the scheduled resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FireTarget {
    /// The fire handler kind (`deployment`, `dream`).
    pub kind: String,
    /// The scheduled resource's id.
    pub id: String,
}

impl FireTarget {
    /// Target `id` of the given handler kind.
    pub fn new(kind: &str, id: impl Into<String>) -> Self {
        Self { kind: kind.to_owned(), id: id.into() }
    }

    /// Stable identity string, used as the schedule id and in `fire_id`.
    pub fn key(&self) -> String {
        format!("{}/{}", self.kind, self.id)
    }
}

/// The tenant a schedule fires under (rides with every fire delivery — a
/// remote backend like Temporal carries it in the workflow payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantRef {
    /// Organization.
    pub org_id: String,
    /// Workspace.
    pub workspace_id: String,
    /// The API key that registered the schedule (the audit actor of its fires).
    pub api_key_id: String,
}

/// A durable schedule registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleSpec {
    /// 5-field cron expression (validated by [`crate::schedule`]).
    pub cron: String,
    /// IANA timezone.
    pub timezone: String,
    /// What to fire.
    pub target: FireTarget,
    /// Whose resource this is.
    pub tenant: TenantRef,
}

impl ScheduleSpec {
    /// Its cron and timezone, ready to compute fire times.
    pub fn compile(&self) -> Result<CompiledSchedule, ScheduleError> {
        let cron = CronSchedule {
            kind: CronKind::Cron,
            expression: self.cron.clone(),
            timezone: self.timezone.clone(),
        };
        cron.compile()
    }
}

/// Idempotency identity of a single fire: `(target, scheduled_at)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FireId {
    /// Target key (see [`FireTarget::key`]).
    pub target: String,
    /// The scheduled wall-clock instant (UTC) of this tick.
    pub scheduled_at: DateTime<Utc>,
}

/// Observed schedule state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleState {
    /// Whether the schedule is currently paused.
    pub paused: bool,
}

/// Errors from the scheduler backend.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SchedulerError {
    /// The spec is invalid (bad cron/timezone).
    #[error("invalid schedule: {0}")]
    InvalidSpec(String),
    /// Unknown schedule id.
    #[error("unknown schedule: {0}")]
    Unknown(String),
    /// Backend unavailable.
    #[error("scheduler unavailable: {0}")]
    Unavailable(String),
}

/// Durable timer backend. Implementations MUST honor the fire contract
/// documented at module level.
#[async_trait]
pub trait SchedulerPort: Send + Sync {
    /// Create or replace the schedule for a target.
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError>;
    /// Suppress scheduled fires (reversible).
    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError>;
    /// Resume from the next occurrence (no backfill).
    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError>;
    /// Remove the schedule permanently.
    async fn delete(&self, target: &FireTarget) -> Result<(), SchedulerError>;
    /// Observe schedule state.
    async fn describe(&self, target: &FireTarget) -> Result<ScheduleState, SchedulerError>;
}

/// One change to a schedule, as the outbox carries it to the scheduler sink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ScheduleChange {
    /// Register or replace the schedule.
    Upsert {
        /// The schedule.
        spec: ScheduleSpec,
    },
    /// Stop its fires until unpaused.
    Pause {
        /// The schedule's target.
        target: FireTarget,
    },
    /// Fire again from the next occurrence.
    Unpause {
        /// The schedule's target.
        target: FireTarget,
    },
    /// Remove it.
    Delete {
        /// The schedule's target.
        target: FireTarget,
    },
}

impl ScheduleChange {
    /// The target the change is about.
    pub fn target(&self) -> &FireTarget {
        match self {
            Self::Upsert { spec } => &spec.target,
            Self::Pause { target } | Self::Unpause { target } | Self::Delete { target } => target,
        }
    }

    /// Apply it to `sink`. A schedule already gone has nothing left to
    /// pause, unpause or delete, so that counts as applied.
    pub async fn apply(&self, sink: &dyn SchedulerPort) -> Result<(), SchedulerError> {
        let applied = match self {
            Self::Upsert { spec } => sink.upsert(spec.clone()).await,
            Self::Pause { target } => sink.pause(target).await,
            Self::Unpause { target } => sink.unpause(target).await,
            Self::Delete { target } => sink.delete(target).await,
        };
        match applied {
            Err(SchedulerError::Unknown(_)) => Ok(()),
            other => other,
        }
    }
}

mod fake;
pub use fake::FakeScheduler;

#[cfg(test)]
mod tests;
