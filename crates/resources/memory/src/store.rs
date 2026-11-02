//! Versioned memory persistence — the append-only chain + movable `latest`
//! ref (the git-object model, in memory; a gix-backed impl can slot in behind
//! the same trait).
//!
//! Invariants:
//! - versions are immutable once written (redact scrubs *content*, never the
//!   audit spine),
//! - the head (`latest`) is a pointer to one version; update advances it,
//!   [`MemoryVersionPersistence::rollback`] repoints it in O(1),
//! - resolve happens through [`VersionSelector`] so callers (sessions,
//!   dreams) choose `latest` or a pinned version explicitly.

use async_trait::async_trait;
use shared::{Pagination, WorkspaceId};

mod filters;

pub use filters::{StoreFilter, VersionFilter};

use crate::types::{Memory, MemoryId, MemoryStoreEntity, MemoryStoreId, MemoryVersion, MemoryVersionId};

/// How a reader resolves a memory to a concrete version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSelector {
    /// The current head — re-resolved at each session-create.
    Latest,
    /// A pinned version.
    Pinned(MemoryVersionId),
}

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such memory store.
    #[error("memory store not found")]
    StoreNotFound,
    /// No such memory.
    #[error("memory not found")]
    MemoryNotFound,
    /// No such version.
    #[error("memory version not found")]
    VersionNotFound,
    /// Create at an occupied path.
    #[error("path conflict")]
    PathConflict {
        /// The occupied path.
        path: String,
        /// Who occupies it.
        memory_id: MemoryId,
    },
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// One version's full record: wire metadata + content blob.
#[derive(Debug, Clone)]
pub struct VersionRecord {
    /// Wire metadata (content field unset; populated on full view).
    pub version: MemoryVersion,
    /// The content blob — `None` after redact.
    pub blob: Option<String>,
}

/// A memory's full state: head pointer + chain.
#[derive(Debug, Clone)]
pub struct MemoryRecord {
    /// Wire head (its `memory_version_id` IS the `latest` ref).
    pub head: Memory,
    /// Whether the memory is deleted (chain retained for audit).
    pub deleted: bool,
    /// The append-only chain, oldest first.
    pub chain: Vec<VersionRecord>,
}

impl MemoryRecord {
    /// Find a version in the chain.
    fn find(&self, id: &MemoryVersionId) -> Option<&VersionRecord> {
        self.chain.iter().find(|v| &v.version.id == id)
    }

    /// Resolve a selector to a version record.
    pub fn resolve(&self, sel: &VersionSelector) -> Option<&VersionRecord> {
        match sel {
            VersionSelector::Latest => self.find(&self.head.memory_version_id),
            VersionSelector::Pinned(id) => self.find(id),
        }
    }
}

/// Persistence port for stores/memories/versions.
pub trait MemoryPersistence:
    MemoryStorePersistence + MemoryRecordPersistence + MemoryVersionPersistence
{
}

impl<T> MemoryPersistence for T where
    T: MemoryStorePersistence + MemoryRecordPersistence + MemoryVersionPersistence
{
}

/// Persistence port for memory stores.
#[async_trait]
pub trait MemoryStorePersistence: Send + Sync {
    /// Insert a store.
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError>;
    /// Fetch a store.
    async fn get_store(&self, ws: &WorkspaceId, id: &MemoryStoreId) -> Result<MemoryStoreEntity, StoreError>;
    /// Replace a store.
    async fn replace_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError>;
    /// Delete a store (and all memories).
    async fn delete_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError>;
    /// All stores.
    async fn list_stores(
        &self,
        ws: &WorkspaceId,
        filter: &StoreFilter,
        page: &Pagination,
    ) -> Result<Vec<MemoryStoreEntity>, StoreError>;
}

/// Persistence port for memory records (head + chain).
#[async_trait]
pub trait MemoryRecordPersistence: Send + Sync {
    /// Insert a brand-new memory (version 1 chain). Fails on occupied path.
    async fn insert_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError>;
    /// Fetch a memory record (head + chain).
    async fn get_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
    ) -> Result<MemoryRecord, StoreError>;
    /// Replace a memory record (append version / advance head / delete mark).
    async fn replace_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError>;
    /// Live (non-deleted) memories in a store, path order.
    async fn list_memories(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path_prefix: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, StoreError>;
    /// Whether a path is occupied by a live memory (for conflict checks).
    async fn find_by_path(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path: &str,
    ) -> Result<Option<MemoryId>, StoreError>;
}

/// Persistence port for memory versions.
#[async_trait]
pub trait MemoryVersionPersistence: Send + Sync {
    /// One page of the store's versions that pass `filter`, newest first,
    /// without content.
    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        filter: &VersionFilter,
        page: &Pagination,
    ) -> Result<Vec<VersionRecord>, StoreError>;
    /// Fetch one version (searches all memories incl. deleted).
    async fn get_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryVersionId,
    ) -> Result<(MemoryId, VersionRecord), StoreError>;

    /// **Extension**: repoint a memory's head to an earlier version — O(1),
    /// non-destructive (no new version is written).
    async fn rollback(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
        to: &MemoryVersionId,
    ) -> Result<MemoryRecord, StoreError>;
}
