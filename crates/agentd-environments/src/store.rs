//! Environment persistence — **org-scoped** (shared across workspaces).

use agentd_core::{OrgId, Pagination};
use async_trait::async_trait;

use crate::types::{Environment, EnvironmentId};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such environment in this org.
    #[error("environment not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] agentd_core::BackendError),
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
    async fn list(&self, org: &OrgId, page: &Pagination) -> Result<Vec<Environment>, StoreError>;
}
