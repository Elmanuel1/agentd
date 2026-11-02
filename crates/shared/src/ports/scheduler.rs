//! The scheduler port: durable timers that fire a registered target.
//!
//! Scheduler fire contract (fixed, backend-independent): **no retry**
//! (`attempts = 1`), **no backfill**, exactly-once per `(target, scheduled_at)`.
//! `upcoming_runs_at` is computed by [`crate::schedule`], never by the backend,
//! so the wire is identical across backends.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── Scheduler port ────────────────────────────────────────────────────────

/// What a schedule fires: the owning resource's [`fire::FireHandler::kind`]
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
    /// Last fire the backend attempted, if any.
    pub last_fire: Option<DateTime<Utc>>,
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

// ─── In-memory fakes (test support, usable by all crates) ─────────────────

/// Deterministic in-memory [`SchedulerPort`] for tests. Records registrations;
/// tests trigger fires manually.
#[derive(Debug, Default)]
pub struct FakeScheduler {
    inner: std::sync::Mutex<BTreeMap<String, (ScheduleSpec, ScheduleState)>>,
}

impl FakeScheduler {
    /// New empty fake.
    pub fn new() -> Self {
        Self::default()
    }

    /// Test hook: all registered specs.
    pub fn registered(&self) -> Vec<ScheduleSpec> {
        self.inner.lock().map(|m| m.values().map(|(s, _)| s.clone()).collect()).unwrap_or_default()
    }
}

#[async_trait]
impl SchedulerPort for FakeScheduler {
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError> {
        // Validate through the shared engine so fakes enforce the same rules.
        crate::schedule::CronSchedule {
            kind: crate::schedule::CronKind::Cron,
            expression: spec.cron.clone(),
            timezone: spec.timezone.clone(),
        }
        .compile()
        .map_err(|e| SchedulerError::InvalidSpec(e.to_string()))?;
        let mut m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        let key = spec.target.key();
        m.insert(key, (spec, ScheduleState { paused: false, last_fire: None }));
        Ok(())
    }

    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        self.with_state(target, |st| st.paused = true).await
    }

    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        self.with_state(target, |st| st.paused = false).await
    }

    async fn delete(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        m.remove(&target.key()).map(|_| ()).ok_or_else(|| SchedulerError::Unknown(target.key()))
    }

    async fn describe(&self, target: &FireTarget) -> Result<ScheduleState, SchedulerError> {
        let m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        m.get(&target.key()).map(|(_, st)| st.clone()).ok_or_else(|| SchedulerError::Unknown(target.key()))
    }
}

impl FakeScheduler {
    async fn with_state(
        &self,
        target: &FireTarget,
        f: impl FnOnce(&mut ScheduleState),
    ) -> Result<(), SchedulerError> {
        let mut m = self.inner.lock().map_err(|e| SchedulerError::Unavailable(e.to_string()))?;
        match m.get_mut(&target.key()) {
            Some((_, st)) => {
                f(st);
                Ok(())
            }
            None => Err(SchedulerError::Unknown(target.key())),
        }
    }
}

#[cfg(test)]
mod tests;
