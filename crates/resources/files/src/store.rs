//! File persistence — workspace-scoped blobs.

use async_trait::async_trait;
use bytes::Bytes;
use shared::{IdCursor, WorkspaceId};

use crate::types::{FileId, FileMetadata};
use crate::FileFilter;

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such file.
    #[error("file not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// A stored file: metadata + blob + optional session scope.
#[derive(Debug, Clone)]
pub struct StoredFile {
    /// Wire metadata.
    pub meta: FileMetadata,
    /// Content on insert; empty on reads, whose bytes come from
    /// [`FilesStore::content`].
    pub bytes: Bytes,
    /// Session scope (outputs written to `/mnt/session/outputs/`).
    pub scope_id: Option<String>,
}

/// Persistence port.
#[async_trait]
pub trait FilesStore: Send + Sync {
    /// Insert.
    async fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> Result<(), StoreError>;
    /// Fetch the metadata (no content).
    async fn get(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError>;
    /// Fetch the content.
    async fn content(&self, ws: &WorkspaceId, id: &FileId) -> Result<Bytes, StoreError>;
    /// Delete.
    async fn delete(&self, ws: &WorkspaceId, id: &FileId) -> Result<StoredFile, StoreError>;
    /// One page of files (id-cursor), optionally scope-filtered. Returns the
    /// page rows plus whether more remain. Pushed DB-side on the default store.
    async fn list(
        &self,
        ws: &WorkspaceId,
        filter: &FileFilter,
        cursor: &IdCursor,
    ) -> Result<(Vec<StoredFile>, bool), StoreError>;
}
