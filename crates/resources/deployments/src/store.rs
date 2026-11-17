//! Deployment persistence — deployments + append-only run records.

use shared::ports::BoxFuture;
use shared::ports::ScheduleChange;
use shared::{Pagination, WorkspaceId};
use std::future::Future;

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
pub trait DeploymentsStore: Send + Sync {
    /// Insert.
    /// Insert `d`, with its schedule's registration in the same transaction.
    fn insert<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        d: Deployment,
        schedule: &'a ScheduleChange,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Fetch.
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a DeploymentId,
    ) -> BoxFuture<'a, Result<Deployment, StoreError>>;
    /// Replace.
    /// Replace `d`, with the change to its schedule, when there is one, in
    /// the same transaction.
    fn replace<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        d: Deployment,
        schedule: Option<&'a ScheduleChange>,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
    /// List, insertion order.
    fn list<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Deployment>, StoreError>>;
    /// Count (for the ≤1000/org limit; keyed by workspace here, org check in api).
    fn count<'a>(&'a self, ws: &'a WorkspaceId) -> BoxFuture<'a, Result<usize, StoreError>>;
    /// Append a run record.
    fn push_run<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        run: DeploymentRun,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Runs for one deployment, newest first, optional error filter.
    fn runs<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        deployment: &'a DeploymentId,
        has_error: Option<bool>,
    ) -> BoxFuture<'a, Result<Vec<DeploymentRun>, StoreError>>;
}

/// What an adapter implements to provide [`DeploymentsStore`]; the methods are documented there.
pub trait NativeDeploymentsStore: Send + Sync {
    fn insert(
        &self,
        ws: &WorkspaceId,
        d: Deployment,
        schedule: &ScheduleChange,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn get(
        &self,
        ws: &WorkspaceId,
        id: &DeploymentId,
    ) -> impl Future<Output = Result<Deployment, StoreError>> + Send;
    fn replace(
        &self,
        ws: &WorkspaceId,
        d: Deployment,
        schedule: Option<&ScheduleChange>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn list(
        &self,
        ws: &WorkspaceId,
        page: &Pagination,
    ) -> impl Future<Output = Result<Vec<Deployment>, StoreError>> + Send;
    fn count(&self, ws: &WorkspaceId) -> impl Future<Output = Result<usize, StoreError>> + Send;
    fn push_run(
        &self,
        ws: &WorkspaceId,
        run: DeploymentRun,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> impl Future<Output = Result<Vec<DeploymentRun>, StoreError>> + Send;
}

impl<T: NativeDeploymentsStore> DeploymentsStore for T {
    fn insert<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        d: Deployment,
        schedule: &'a ScheduleChange,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeDeploymentsStore::insert(self, ws, d, schedule))
    }
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a DeploymentId,
    ) -> BoxFuture<'a, Result<Deployment, StoreError>> {
        Box::pin(NativeDeploymentsStore::get(self, ws, id))
    }
    fn replace<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        d: Deployment,
        schedule: Option<&'a ScheduleChange>,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeDeploymentsStore::replace(self, ws, d, schedule))
    }
    fn list<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Deployment>, StoreError>> {
        Box::pin(NativeDeploymentsStore::list(self, ws, page))
    }
    fn count<'a>(&'a self, ws: &'a WorkspaceId) -> BoxFuture<'a, Result<usize, StoreError>> {
        Box::pin(NativeDeploymentsStore::count(self, ws))
    }
    fn push_run<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        run: DeploymentRun,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeDeploymentsStore::push_run(self, ws, run))
    }
    fn runs<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        deployment: &'a DeploymentId,
        has_error: Option<bool>,
    ) -> BoxFuture<'a, Result<Vec<DeploymentRun>, StoreError>> {
        Box::pin(NativeDeploymentsStore::runs(self, ws, deployment, has_error))
    }
}
