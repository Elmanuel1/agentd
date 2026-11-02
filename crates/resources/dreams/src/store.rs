//! Dream persistence.

use async_trait::async_trait;
use shared::WorkspaceId;

use crate::types::{Dream, DreamId, DreamRun};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such dream.
    #[error("dream not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port.
#[async_trait]
pub trait DreamsStore: Send + Sync {
    /// Insert.
    async fn insert(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError>;
    /// Fetch.
    async fn get(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Dream, StoreError>;
    /// Replace.
    async fn replace(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError>;
    /// List.
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Dream>, StoreError>;
    /// Append a run.
    async fn push_run(&self, ws: &WorkspaceId, run: DreamRun) -> Result<(), StoreError>;
    /// Runs newest first.
    async fn runs(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Vec<DreamRun>, StoreError>;
}
