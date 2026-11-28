//! API key persistence. A key is found by its digest; the digest is written
//! once per pepper and never read back.

use async_trait::async_trait;
use shared::{OrgId, Timestamp, WorkspaceId};

use crate::types::{AdminKey, ApiKey, EnvironmentKey};

/// Store-level errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// No organization has the key's org id.
    #[error("no organization {0}")]
    UnknownOrganization(String),
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Keys of one kind, found by digest.
#[async_trait]
pub trait KeyDigests<K>: Send + Sync {
    /// The unrevoked key stored under one of `digests`, and the digest it is
    /// stored under.
    async fn find_active(&self, digests: &[Vec<u8>]) -> Result<Option<(K, Vec<u8>)>, StoreError>;
    /// Store key `id` under `digest` instead.
    async fn rehash(&self, id: &str, digest: &[u8]) -> Result<(), StoreError>;
}

/// Persistence port for workspace API keys.
#[async_trait]
pub trait ApiKeysStore: KeyDigests<ApiKey> {
    /// Insert a new key with its digest.
    async fn insert(&self, key: &ApiKey, digest: &[u8]) -> Result<(), StoreError>;
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

/// Persistence port for environment keys.
#[async_trait]
pub trait EnvironmentKeysStore: KeyDigests<EnvironmentKey> {
    /// Insert a new key with its digest.
    async fn insert(&self, key: &EnvironmentKey, digest: &[u8]) -> Result<(), StoreError>;
    /// Every key, revoked ones included, oldest first.
    async fn list(&self) -> Result<Vec<EnvironmentKey>, StoreError>;
    /// Revoke `id` at `at`; `false` when no unrevoked key has it.
    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError>;
}

/// Persistence port for organization admin keys.
#[async_trait]
pub trait AdminKeysStore: KeyDigests<AdminKey> {
    /// Insert a new key with its digest.
    async fn insert(&self, key: &AdminKey, digest: &[u8]) -> Result<(), StoreError>;
    /// Every key, revoked ones included, oldest first.
    async fn list(&self) -> Result<Vec<AdminKey>, StoreError>;
    /// Revoke `id` at `at`; `false` when no unrevoked key has it.
    async fn revoke(&self, id: &str, at: Timestamp) -> Result<bool, StoreError>;
}
