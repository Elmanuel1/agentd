//! File persistence — workspace-scoped blobs.

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;

use crate::types::{FileId, FileMetadata};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such file.
    #[error("file not found")]
    NotFound,
}

/// A stored file: metadata + blob + optional session scope.
#[derive(Debug, Clone)]
pub struct StoredFile {
    /// Wire metadata.
    pub meta: FileMetadata,
    /// Content.
    pub bytes: Bytes,
    /// Session scope (outputs written to `/mnt/session/outputs/`).
    pub scope_id: Option<String>,
}

/// Persistence port.
#[async_trait]
pub trait FilesStore: Send + Sync {
    /// Insert.
    async fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> Result<(), StoreError>;
    /// Fetch.
    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError>;
    /// Delete.
    async fn delete(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError>;
    /// List, optionally filtered by scope.
    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
    ) -> Result<Vec<StoredFile>, StoreError>;
}

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemoryFilesStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<FileId, StoredFile>>>,
}

impl InMemoryFilesStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FilesStore for InMemoryFilesStore {
    async fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(f.meta.id.clone(), f);
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn delete(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError> {
        self.inner
            .write()
            .get_mut(ws)
            .and_then(|m| m.remove(id))
            .ok_or(StoreError::NotFound)
    }

    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
    ) -> Result<Vec<StoredFile>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| {
                m.values()
                    .filter(|f| scope_id.is_none_or(|s| f.scope_id.as_deref() == Some(s)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }
}
