//! A deterministic in-memory [`Runtime`] for tests.

use std::collections::BTreeMap;

use async_trait::async_trait;

#[cfg(test)]
use super::JobKind;
use super::{JobSpec, Runtime, RuntimeError, RuntimeHandle, TaskState};

/// Deterministic in-memory [`Runtime`] for tests.
#[derive(Debug, Default)]
pub struct FakeRuntime {
    inner: std::sync::Mutex<FakeRuntimeState>,
}

#[derive(Debug, Default)]
struct FakeRuntimeState {
    counter: u64,
    tasks: BTreeMap<String, (JobSpec, TaskState)>,
    /// What each stop answers instead of stopping, when set.
    failing: Option<RuntimeError>,
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

    /// Test hook: every stop fails with `error` until it is cleared.
    pub fn fail_stops(&self, error: Option<RuntimeError>) {
        if let Ok(mut s) = self.inner.lock() {
            s.failing = error;
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
        if let Some(e) = s.failing.clone() {
            return Err(e);
        }
        match s.tasks.get_mut(&handle.id) {
            Some(entry) => {
                entry.1 = TaskState::Exited(0);
                Ok(())
            }
            None => Err(RuntimeError::Unknown(handle.id.clone())),
        }
    }

    fn handle_for(&self, target_id: &str) -> Option<RuntimeHandle> {
        let s = self.inner.lock().ok()?;
        let (id, _) = s.tasks.iter().find(|(_, (job, _))| job.target_id == target_id)?;
        Some(RuntimeHandle { id: id.clone() })
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
            environment: serde_json::Value::Null,
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
