//! File persistence — workspace-scoped blobs.

use agentd_core::{IdCursor, WorkspaceId};
use async_trait::async_trait;
use bytes::Bytes;

use crate::types::{FileId, FileMetadata};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such file.
    #[error("file not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] agentd_core::BackendError),
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
    /// One page of files (id-cursor), optionally scope-filtered. Returns the
    /// page rows plus whether more remain. Pushed DB-side on the default store.
    async fn list(
        &self,
        ws: &WorkspaceId,
        scope_id: Option<&str>,
        cursor: &IdCursor,
    ) -> Result<(Vec<StoredFile>, bool), StoreError>;
}
