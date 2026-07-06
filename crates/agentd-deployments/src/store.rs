//! Deployment persistence — deployments + append-only run records.

use agentd_core::{Pagination, WorkspaceId};
use async_trait::async_trait;

use crate::types::{Deployment, DeploymentId, DeploymentRun};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such deployment.
    #[error("deployment not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] agentd_core::BackendError),
}

/// Persistence port.
#[async_trait]
pub trait DeploymentsStore: Send + Sync {
    /// Insert.
    async fn insert(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError>;
    /// Fetch.
    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError>;
    /// Replace.
    async fn replace(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError>;
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

    /// Atomically claim one `(deployment, scheduled_at)` fire.
    ///
    /// Returns `true` if this call won the claim (the caller should fire),
    /// `false` if another replica already claimed it (skip). This is the
    /// exactly-once boundary across replicas — it MUST be atomic. The
    /// Postgres impl is `INSERT … ON CONFLICT DO NOTHING` on a
    /// `UNIQUE(deployment_id, scheduled_at)` index (rows-affected = the
    /// boolean). App code must never do a read-then-write dup check.
    async fn claim_fire(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        scheduled_at: &str,
    ) -> Result<bool, StoreError>;
}
