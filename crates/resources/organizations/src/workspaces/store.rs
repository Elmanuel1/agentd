//! Workspace persistence, org-scoped.

use async_trait::async_trait;
use shared::{IdCursor, OrgId};

use crate::workspaces::types::Workspace;

/// Store-level errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// No such workspace in this org.
    #[error("workspace not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port for workspaces.
#[async_trait]
pub trait WorkspacesStore: Send + Sync {
    /// Insert a new workspace.
    async fn insert(&self, org: &OrgId, workspace: &Workspace) -> Result<(), StoreError>;
    /// Fetch one.
    async fn get(&self, org: &OrgId, id: &str) -> Result<Workspace, StoreError>;
    /// Replace one.
    async fn replace(&self, org: &OrgId, workspace: &Workspace) -> Result<(), StoreError>;
    /// One page, newest first, and whether more follow in the page's direction:
    /// after `after_id` going older, or before `before_id` going newer. Archived
    /// workspaces only with `include_archived`.
    async fn list(
        &self,
        org: &OrgId,
        include_archived: bool,
        cursor: &IdCursor,
    ) -> Result<(Vec<Workspace>, bool), StoreError>;
}
