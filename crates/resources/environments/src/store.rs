//! Environment persistence — **org-scoped** (shared across workspaces).

use async_trait::async_trait;
use shared::{OrgId, Pagination};

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
#[async_trait]
pub trait EnvironmentsStore: Send + Sync {
    /// Insert a new environment.
    async fn insert(&self, org: &OrgId, env: Environment) -> Result<(), StoreError>;
    /// Fetch one.
    async fn get(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError>;
    /// Replace one.
    async fn replace(&self, org: &OrgId, env: Environment) -> Result<(), StoreError>;
    /// Remove one.
    async fn delete(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError>;
    /// All environments in the org, insertion order.
    /// One page, newest first; archived environments only with `include_archived`.
    async fn list(
        &self,
        org: &OrgId,
        include_archived: bool,
        page: &Pagination,
    ) -> Result<Vec<Environment>, StoreError>;
}
