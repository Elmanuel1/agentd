//! Swappable external ports. Each backend implements one trait and is chosen
//! by config at the composition root; the in-memory fakes make every consumer
//! testable.
//!
//! Scheduler fire contract (fixed, backend-independent): **no retry**
//! (`attempts = 1`), **no backfill**, exactly-once per `(target, scheduled_at)`.
//! `upcoming_runs_at` is computed by [`crate::schedule`], never by the backend,
//! so the wire is identical across backends.

pub mod blobs;
pub mod fire;
pub mod keys;
pub mod secrets;

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ─── Runtime port ──────────────────────────────────────────────────────────

/// What kind of job an ephemeral task runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// An interactive/autonomous agent session.
    Session,
    /// A dream (offline reflection) run.
    Dream,
}

/// Specification for launching one ephemeral engine job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSpec {
    /// Job kind.
    pub kind: JobKind,
    /// The session (or dream-run) id this job serves.
    pub target_id: String,
    /// Workspace the job belongs to.
    pub workspace_id: String,
    /// Environment id (resolves image/networking template).
    pub environment_id: String,
    /// Extra environment variables for the task (non-secret; secrets go
    /// through vault egress, never into the task).
    pub env: BTreeMap<String, String>,
}

/// Handle to a launched task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHandle {
    /// Backend-specific task identifier (ECS task ARN, VM id, container id…).
    pub id: String,
}

/// Observed task state, used by reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Queued/provisioning.
    Pending,
    /// Running.
    Running,
    /// Exited with a code.
    Exited(i32),
    /// Backend lost or failed the task.
    Failed(String),
}

/// Errors from the runtime backend.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RuntimeError {
    /// Task could not be launched.
    #[error("launch failed: {0}")]
    Launch(String),
    /// Unknown handle.
    #[error("unknown task: {0}")]
    Unknown(String),
    /// Backend unavailable.
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
}

/// Sandbox runtime for ephemeral engine jobs.
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Launch one job; returns a handle for status/stop.
    async fn launch(&self, job: JobSpec) -> Result<RuntimeHandle, RuntimeError>;
    /// Observe current task state (reconciliation / orphan detection).
    async fn status(&self, handle: &RuntimeHandle) -> Result<TaskState, RuntimeError>;
    /// Stop a task.
    async fn stop(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError>;
}

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

/// Deterministic in-memory [`Runtime`] for tests.
#[derive(Debug, Default)]
pub struct FakeRuntime {
    inner: std::sync::Mutex<FakeRuntimeState>,
}

#[derive(Debug, Default)]
struct FakeRuntimeState {
    counter: u64,
    tasks: BTreeMap<String, (JobSpec, TaskState)>,
}

impl FakeRuntime {
    /// New empty fake.
    pub fn new() -> Self {
        Self::default()
    }

    /// Test hook: force a task into a state (e.g. simulate a crash).
    pub fn set_state(&self, handle: &RuntimeHandle, state: TaskState) {
        if let Ok(mut s) = self.inner.lock() {
            if let Some(entry) = s.tasks.get_mut(&handle.id) {
                entry.1 = state;
            }
        }
    }

    /// Test hook: inspect launched jobs.
    pub fn launched(&self) -> Vec<JobSpec> {
        self.inner.lock().map(|s| s.tasks.values().map(|(j, _)| j.clone()).collect()).unwrap_or_default()
    }
}

#[async_trait]
impl Runtime for FakeRuntime {
    async fn launch(&self, job: JobSpec) -> Result<RuntimeHandle, RuntimeError> {
        let mut s = self.inner.lock().map_err(|e| RuntimeError::Unavailable(e.to_string()))?;
        s.counter += 1;
        let id = format!("fake-task-{}", s.counter);
        s.tasks.insert(id.clone(), (job, TaskState::Running));
        Ok(RuntimeHandle { id })
    }

    async fn status(&self, handle: &RuntimeHandle) -> Result<TaskState, RuntimeError> {
        let s = self.inner.lock().map_err(|e| RuntimeError::Unavailable(e.to_string()))?;
        s.tasks
            .get(&handle.id)
            .map(|(_, st)| st.clone())
            .ok_or_else(|| RuntimeError::Unknown(handle.id.clone()))
    }

    async fn stop(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError> {
        let mut s = self.inner.lock().map_err(|e| RuntimeError::Unavailable(e.to_string()))?;
        match s.tasks.get_mut(&handle.id) {
            Some(entry) => {
                entry.1 = TaskState::Exited(0);
                Ok(())
            }
            None => Err(RuntimeError::Unknown(handle.id.clone())),
        }
    }
}

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
mod tests {
    use super::*;

    fn job() -> JobSpec {
        JobSpec {
            kind: JobKind::Session,
            target_id: "sesn_01x".into(),
            workspace_id: "wrkspc_01x".into(),
            environment_id: "env_01x".into(),
            env: BTreeMap::new(),
        }
    }

    #[tokio::test]
    async fn fake_runtime_lifecycle() {
        let rt = FakeRuntime::new();
        let h = rt.launch(job()).await.unwrap();
        assert_eq!(rt.status(&h).await.unwrap(), TaskState::Running);
        rt.stop(&h).await.unwrap();
        assert_eq!(rt.status(&h).await.unwrap(), TaskState::Exited(0));
        assert_eq!(rt.launched().len(), 1);
    }

    #[tokio::test]
    async fn fake_runtime_orphan_simulation() {
        let rt = FakeRuntime::new();
        let h = rt.launch(job()).await.unwrap();
        rt.set_state(&h, TaskState::Failed("oom".into()));
        assert!(matches!(rt.status(&h).await.unwrap(), TaskState::Failed(_)));
    }

    #[tokio::test]
    async fn fake_runtime_unknown_handle() {
        let rt = FakeRuntime::new();
        let bogus = RuntimeHandle { id: "nope".into() };
        assert!(matches!(rt.status(&bogus).await, Err(RuntimeError::Unknown(_))));
    }

    #[tokio::test]
    async fn fake_scheduler_lifecycle() {
        let s = FakeScheduler::new();
        let target = FireTarget::new("deployment", "depl_01x");
        s.upsert(ScheduleSpec {
            cron: "0 0 * * *".into(),
            timezone: "UTC".into(),
            target: target.clone(),
            tenant: tenant(),
        })
        .await
        .unwrap();
        assert!(!s.describe(&target).await.unwrap().paused);
        s.pause(&target).await.unwrap();
        assert!(s.describe(&target).await.unwrap().paused);
        s.unpause(&target).await.unwrap();
        assert!(!s.describe(&target).await.unwrap().paused);
        s.delete(&target).await.unwrap();
        assert!(matches!(s.describe(&target).await, Err(SchedulerError::Unknown(_))));
    }

    #[tokio::test]
    async fn fake_scheduler_validates_cron_via_shared_engine() {
        let s = FakeScheduler::new();
        let bad = s
            .upsert(ScheduleSpec {
                cron: "99 * * * *".into(),
                timezone: "UTC".into(),
                target: FireTarget::new("dream", "d1"),
                tenant: tenant(),
            })
            .await;
        assert!(matches!(bad, Err(SchedulerError::InvalidSpec(_))));
    }

    fn tenant() -> TenantRef {
        TenantRef {
            org_id: "org_t".into(),
            workspace_id: "wrkspc_01t".into(),
            api_key_id: "apikey_01t".into(),
        }
    }
}
