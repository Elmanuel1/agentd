//! Runtime-port implementation for local execution.
//!
//! [`LocalTokioRuntime`] runs each job as an in-process tokio task — the
//! development/self-hosted backend. Fargate/Firecracker/CloudRun implement
//! the same `agentd_core::ports::Runtime` trait at the deployment layer;
//! the port contract (launch/status/stop) is what makes them drop-ins.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use agentd_core::ports::{JobSpec, Runtime, RuntimeError, RuntimeHandle, TaskState};
use async_trait::async_trait;
use parking_lot::Mutex;

/// The job body a launched task runs.
pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;
/// Builds the job body from the spec (wired by agentd-api with the real
/// TurnRunner; tests inject scripted futures).
pub type JobFactory = Arc<dyn Fn(JobSpec) -> JobFuture + Send + Sync>;

/// In-process runtime: one tokio task per job.
pub struct LocalTokioRuntime {
    factory: JobFactory,
    tasks: Mutex<BTreeMap<String, tokio::task::JoinHandle<Result<(), String>>>>,
    counter: Mutex<u64>,
}

impl std::fmt::Debug for LocalTokioRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTokioRuntime").finish_non_exhaustive()
    }
}

impl LocalTokioRuntime {
    /// Build with a job factory.
    pub fn new(factory: JobFactory) -> Self {
        Self { factory, tasks: Mutex::new(BTreeMap::new()), counter: Mutex::new(0) }
    }
}

#[async_trait]
impl Runtime for LocalTokioRuntime {
    async fn launch(&self, job: JobSpec) -> Result<RuntimeHandle, RuntimeError> {
        let fut = (self.factory)(job);
        let handle = tokio::spawn(fut);
        let id = {
            let mut c = self.counter.lock();
            *c += 1;
            format!("local-task-{}", *c)
        };
        self.tasks.lock().insert(id.clone(), handle);
        Ok(RuntimeHandle { id })
    }

    async fn status(&self, handle: &RuntimeHandle) -> Result<TaskState, RuntimeError> {
        // Resolve everything under the lock in a sync scope, then await
        // OUTSIDE it (a parking_lot guard is not Send across await points).
        let finished_task = {
            let mut g = self.tasks.lock();
            match g.get(&handle.id) {
                None => return Err(RuntimeError::Unknown(handle.id.clone())),
                Some(task) if !task.is_finished() => return Ok(TaskState::Running),
                Some(_) => g.remove(&handle.id).ok_or_else(|| RuntimeError::Unknown(handle.id.clone()))?,
            }
        };
        match finished_task.await {
            Ok(Ok(())) => Ok(TaskState::Exited(0)),
            Ok(Err(e)) => Ok(TaskState::Failed(e)),
            Err(join) => Ok(TaskState::Failed(format!("task panicked/cancelled: {join}"))),
        }
    }

    async fn stop(&self, handle: &RuntimeHandle) -> Result<(), RuntimeError> {
        let mut g = self.tasks.lock();
        let Some(task) = g.remove(&handle.id) else {
            return Err(RuntimeError::Unknown(handle.id.clone()));
        };
        task.abort();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap as Map;

    fn spec() -> JobSpec {
        JobSpec {
            kind: agentd_core::ports::JobKind::Session,
            target_id: "sesn_01x".into(),
            workspace_id: "wrkspc_01x".into(),
            environment_id: "env_01x".into(),
            env: Map::new(),
        }
    }

    #[tokio::test]
    async fn launch_completes_and_reports_exit() {
        let rt = LocalTokioRuntime::new(Arc::new(|_job| Box::pin(async { Ok(()) }) as JobFuture));
        let h = rt.launch(spec()).await.unwrap();
        // Let the task finish.
        tokio::task::yield_now().await;
        // Poll until finished (bounded).
        let mut state = rt.status(&h).await.unwrap();
        for _ in 0..100 {
            if state != TaskState::Running {
                break;
            }
            tokio::task::yield_now().await;
            state = rt.status(&h).await.unwrap();
        }
        assert_eq!(state, TaskState::Exited(0));
    }

    #[tokio::test]
    async fn failing_job_reports_failed() {
        let rt =
            LocalTokioRuntime::new(Arc::new(|_job| Box::pin(async { Err("boom".to_owned()) }) as JobFuture));
        let h = rt.launch(spec()).await.unwrap();
        tokio::task::yield_now().await;
        let mut state = rt.status(&h).await.unwrap();
        for _ in 0..100 {
            if state != TaskState::Running {
                break;
            }
            tokio::task::yield_now().await;
            state = rt.status(&h).await.unwrap();
        }
        assert_eq!(state, TaskState::Failed("boom".into()));
    }

    #[tokio::test]
    async fn stop_aborts_a_running_job() {
        let rt = LocalTokioRuntime::new(Arc::new(|_job| {
            Box::pin(async {
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                Ok(())
            }) as JobFuture
        }));
        let h = rt.launch(spec()).await.unwrap();
        assert_eq!(rt.status(&h).await.unwrap(), TaskState::Running);
        rt.stop(&h).await.unwrap();
        assert!(matches!(rt.status(&h).await, Err(RuntimeError::Unknown(_))));
    }
}
