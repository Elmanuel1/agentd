//! Workspace API key persistence. A key is found by its digest; the digest is
//! written once and never read back.

use async_trait::async_trait;
use shared::{OrgId, Timestamp, WorkspaceId};

use crate::types::ApiKey;

/// Store-level errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port for workspace API keys.
#[async_trait]
pub trait ApiKeysStore: Send + Sync {
    /// Insert a new key with its digest.
    async fn insert(&self, key: &ApiKey, digest: &[u8]) -> Result<(), StoreError>;
    /// The unrevoked key whose digest is `digest`.
    async fn find_active(&self, digest: &[u8]) -> Result<Option<ApiKey>, StoreError>;
    /// Every key, revoked ones included, oldest first.
    async fn list(&self) -> Result<Vec<ApiKey>, StoreError>;
    /// Revoke `id` at `at`; `false` when no unrevoked key has it.
    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError>;
    /// Revoke every unrevoked key of `workspace` in `org` at `at`; how many.
    async fn revoke_workspace(
        &self,
        org: &OrgId,
        workspace: &WorkspaceId,
        at: Timestamp,
    ) -> Result<usize, StoreError>;
}
