//! Deployment persistence — deployments + append-only run records.

use async_trait::async_trait;
use shared::ports::ScheduleChange;
use shared::{Pagination, WorkspaceId};

use crate::types::{Deployment, DeploymentId, DeploymentRun};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such deployment.
    #[error("deployment not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port.
#[async_trait]
pub trait DeploymentsStore: Send + Sync {
    /// Insert.
    /// Insert `d`, with its schedule's registration in the same transaction.
    async fn insert(
        &self,
        ws: &WorkspaceId,
        d: Deployment,
        schedule: &ScheduleChange,
    ) -> Result<(), StoreError>;
    /// Fetch.
    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError>;
    /// Replace.
    /// Replace `d`, with the change to its schedule, when there is one, in
    /// the same transaction.
    async fn replace(
        &self,
        ws: &WorkspaceId,
        d: Deployment,
        schedule: Option<&ScheduleChange>,
    ) -> Result<(), StoreError>;
    /// List, insertion order.
    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Deployment>, StoreError>;
    /// Count (for the ≤1000/org limit; keyed by workspace here, org check in api).
    async fn count(&self, ws: &WorkspaceId) -> Result<usize, StoreError>;
    /// Append a run record.
    async fn push_run(&self, ws: &WorkspaceId, run: DeploymentRun) -> Result<(), StoreError>;
    /// Runs for one deployment, newest first, optional error filter.
    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError>;
}
