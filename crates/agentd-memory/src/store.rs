//! Versioned memory persistence — the append-only chain + movable `latest`
//! ref (the git-object model, in memory; a gix-backed impl can slot in behind
//! the same trait).
//!
//! Invariants:
//! - versions are immutable once written (redact scrubs *content*, never the
//!   audit spine),
//! - the head (`latest`) is a pointer to one version; update advances it,
//!   [`MemoryPersistence::rollback`] repoints it in O(1),
//! - resolve happens through [`VersionSelector`] so callers (sessions,
//!   dreams) choose `latest` or a pinned version explicitly.

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::types::{
    Memory, MemoryId, MemoryStoreEntity, MemoryStoreId, MemoryVersion, MemoryVersionId,
};

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
    Backend(String),
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
    pub fn find(&self, id: &MemoryVersionId) -> Option<&VersionRecord> {
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
#[async_trait]
pub trait MemoryPersistence: Send + Sync {
    /// Insert a store.
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError>;
    /// Fetch a store.
    async fn get_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError>;
    /// Replace a store.
    async fn replace_store(
        &self,
        ws: &WorkspaceId,
        s: MemoryStoreEntity,
    ) -> Result<(), StoreError>;
    /// Delete a store (and all memories).
    async fn delete_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError>;
    /// All stores.
    async fn list_stores(&self, ws: &WorkspaceId) -> Result<Vec<MemoryStoreEntity>, StoreError>;

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
    /// All versions across the store, newest first, optionally filtered.
    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: Option<&MemoryId>,
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

type StoreEntry = (MemoryStoreEntity, BTreeMap<MemoryId, MemoryRecord>);

/// In-memory implementation.
#[derive(Debug, Default)]
pub struct InMemoryMemoryStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<MemoryStoreId, StoreEntry>>>,
}

impl InMemoryMemoryStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl MemoryPersistence for InMemoryMemoryStore {
    async fn insert_store(&self, ws: &WorkspaceId, s: MemoryStoreEntity) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(s.id.clone(), (s, BTreeMap::new()));
        Ok(())
    }

    async fn get_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(s, _)| s.clone())
            .ok_or(StoreError::StoreNotFound)
    }

    async fn replace_store(
        &self,
        ws: &WorkspaceId,
        s: MemoryStoreEntity,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&s.id))
            .ok_or(StoreError::StoreNotFound)?;
        entry.0 = s;
        Ok(())
    }

    async fn delete_store(
        &self,
        ws: &WorkspaceId,
        id: &MemoryStoreId,
    ) -> Result<MemoryStoreEntity, StoreError> {
        self.inner
            .write()
            .get_mut(ws)
            .and_then(|m| m.remove(id))
            .map(|(s, _)| s)
            .ok_or(StoreError::StoreNotFound)
    }

    async fn list_stores(&self, ws: &WorkspaceId) -> Result<Vec<MemoryStoreEntity>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| m.values().map(|(s, _)| s.clone()).collect())
            .unwrap_or_default())
    }

    async fn insert_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(store))
            .ok_or(StoreError::StoreNotFound)?;
        // Path conflict against live memories.
        if let Some((occupied_id, _)) = entry
            .1
            .iter()
            .find(|(_, r)| !r.deleted && r.head.path == record.head.path)
        {
            return Err(StoreError::PathConflict {
                path: record.head.path.clone(),
                memory_id: occupied_id.clone(),
            });
        }
        entry.1.insert(record.head.id.clone(), record);
        Ok(())
    }

    async fn get_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
    ) -> Result<MemoryRecord, StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(store))
            .ok_or(StoreError::StoreNotFound)?;
        entry
            .1
            .get(id)
            .filter(|r| !r.deleted)
            .cloned()
            .ok_or(StoreError::MemoryNotFound)
    }

    async fn replace_memory(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        record: MemoryRecord,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(store))
            .ok_or(StoreError::StoreNotFound)?;
        if !entry.1.contains_key(&record.head.id) {
            return Err(StoreError::MemoryNotFound);
        }
        // Rename conflict: another live memory already at the new path.
        if let Some((other, _)) = entry.1.iter().find(|(oid, r)| {
            **oid != record.head.id && !r.deleted && r.head.path == record.head.path
        }) {
            if !record.deleted {
                return Err(StoreError::PathConflict {
                    path: record.head.path.clone(),
                    memory_id: other.clone(),
                });
            }
        }
        entry.1.insert(record.head.id.clone(), record);
        Ok(())
    }

    async fn list_memories(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path_prefix: Option<&str>,
    ) -> Result<Vec<MemoryRecord>, StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(store))
            .ok_or(StoreError::StoreNotFound)?;
        let mut out: Vec<MemoryRecord> = entry
            .1
            .values()
            .filter(|r| !r.deleted)
            .filter(|r| path_prefix.is_none_or(|p| r.head.path.starts_with(p)))
            .cloned()
            .collect();
        out.sort_by(|a, b| a.head.path.cmp(&b.head.path));
        Ok(out)
    }

    async fn find_by_path(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        path: &str,
    ) -> Result<Option<MemoryId>, StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(store))
            .ok_or(StoreError::StoreNotFound)?;
        Ok(entry
            .1
            .iter()
            .find(|(_, r)| !r.deleted && r.head.path == path)
            .map(|(id, _)| id.clone()))
    }

    async fn list_versions(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        memory_id: Option<&MemoryId>,
    ) -> Result<Vec<VersionRecord>, StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(store))
            .ok_or(StoreError::StoreNotFound)?;
        let mut out: Vec<VersionRecord> = entry
            .1
            .values()
            .filter(|r| memory_id.is_none_or(|id| &r.head.id == id))
            .flat_map(|r| r.chain.iter().cloned())
            .collect();
        // Newest first.
        out.sort_by(|a, b| b.version.created_at.cmp(&a.version.created_at));
        Ok(out)
    }

    async fn get_version(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryVersionId,
    ) -> Result<(MemoryId, VersionRecord), StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(store))
            .ok_or(StoreError::StoreNotFound)?;
        for r in entry.1.values() {
            if let Some(v) = r.find(id) {
                return Ok((r.head.id.clone(), v.clone()));
            }
        }
        Err(StoreError::VersionNotFound)
    }

    async fn rollback(
        &self,
        ws: &WorkspaceId,
        store: &MemoryStoreId,
        id: &MemoryId,
        to: &MemoryVersionId,
    ) -> Result<MemoryRecord, StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(store))
            .ok_or(StoreError::StoreNotFound)?;
        let record = entry
            .1
            .get_mut(id)
            .filter(|r| !r.deleted)
            .ok_or(StoreError::MemoryNotFound)?;
        let target = record.find(to).cloned().ok_or(StoreError::VersionNotFound)?;
        // O(1) ref move: head now points at the older version; content
        // metadata reflects that version. No new version is written.
        record.head.memory_version_id = target.version.id.clone();
        if let (Some(sha), Some(size), Some(path)) = (
            target.version.content_sha256.clone(),
            target.version.content_size_bytes,
            target.version.path.clone(),
        ) {
            record.head.content_sha256 = sha;
            record.head.content_size_bytes = size;
            record.head.path = path;
        }
        Ok(record.clone())
    }
}
