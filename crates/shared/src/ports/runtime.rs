//! The sandbox runtime port: one ephemeral task per engine job.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

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
}
