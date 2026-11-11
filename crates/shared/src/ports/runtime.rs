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
    /// The environment's `config` as resolved when the job was made (its
    /// `networking` and `packages`).
    #[serde(default)]
    pub environment: serde_json::Value,
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
    /// The handle of `target_id`'s task, for a backend that names each task
    /// after its target and so finds it again after a restart.
    fn handle_for(&self, _target_id: &str) -> Option<RuntimeHandle> {
        None
    }
}

mod fake;

pub use fake::FakeRuntime;
