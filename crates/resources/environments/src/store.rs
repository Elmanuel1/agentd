//! Environment persistence — **org-scoped** (shared across workspaces).

use shared::ports::BoxFuture;
use shared::{OrgId, Pagination};
use std::future::Future;

use crate::types::{Environment, EnvironmentId};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such environment in this org.
    #[error("environment not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port for environments.
pub trait EnvironmentsStore: Send + Sync {
    /// Insert a new environment.
    fn insert<'a>(&'a self, org: &'a OrgId, env: Environment) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Fetch one.
    fn get<'a>(
        &'a self,
        org: &'a OrgId,
        id: &'a EnvironmentId,
    ) -> BoxFuture<'a, Result<Environment, StoreError>>;
    /// Replace one.
    fn replace<'a>(&'a self, org: &'a OrgId, env: Environment) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Remove one.
    fn delete<'a>(
        &'a self,
        org: &'a OrgId,
        id: &'a EnvironmentId,
    ) -> BoxFuture<'a, Result<Environment, StoreError>>;
    /// All environments in the org, insertion order.
    /// One page, newest first; archived environments only with `include_archived`.
    fn list<'a>(
        &'a self,
        org: &'a OrgId,
        include_archived: bool,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Environment>, StoreError>>;
}

/// What an adapter implements to provide [`EnvironmentsStore`]; the methods are documented there.
pub trait NativeEnvironmentsStore: Send + Sync {
    fn insert(&self, org: &OrgId, env: Environment) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn get(
        &self,
        org: &OrgId,
        id: &EnvironmentId,
    ) -> impl Future<Output = Result<Environment, StoreError>> + Send;
    fn replace(&self, org: &OrgId, env: Environment) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn delete(
        &self,
        org: &OrgId,
        id: &EnvironmentId,
    ) -> impl Future<Output = Result<Environment, StoreError>> + Send;
    fn list(
        &self,
        org: &OrgId,
        include_archived: bool,
        page: &Pagination,
    ) -> impl Future<Output = Result<Vec<Environment>, StoreError>> + Send;
}

impl<T: NativeEnvironmentsStore> EnvironmentsStore for T {
    fn insert<'a>(&'a self, org: &'a OrgId, env: Environment) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeEnvironmentsStore::insert(self, org, env))
    }
    fn get<'a>(
        &'a self,
        org: &'a OrgId,
        id: &'a EnvironmentId,
    ) -> BoxFuture<'a, Result<Environment, StoreError>> {
        Box::pin(NativeEnvironmentsStore::get(self, org, id))
    }
    fn replace<'a>(&'a self, org: &'a OrgId, env: Environment) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeEnvironmentsStore::replace(self, org, env))
    }
    fn delete<'a>(
        &'a self,
        org: &'a OrgId,
        id: &'a EnvironmentId,
    ) -> BoxFuture<'a, Result<Environment, StoreError>> {
        Box::pin(NativeEnvironmentsStore::delete(self, org, id))
    }
    fn list<'a>(
        &'a self,
        org: &'a OrgId,
        include_archived: bool,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Environment>, StoreError>> {
        Box::pin(NativeEnvironmentsStore::list(self, org, include_archived, page))
    }
}
