//! File persistence — workspace-scoped blobs.

use bytes::Bytes;
use shared::ports::BoxFuture;
use shared::{IdCursor, WorkspaceId};
use std::future::Future;

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
pub trait FilesStore: Send + Sync {
    /// Insert.
    fn insert<'a>(&'a self, ws: &'a WorkspaceId, f: StoredFile) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Fetch the metadata (no content).
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a FileId,
    ) -> BoxFuture<'a, Result<StoredFile, StoreError>>;
    /// Fetch the content.
    fn content<'a>(&'a self, ws: &'a WorkspaceId, id: &'a FileId)
        -> BoxFuture<'a, Result<Bytes, StoreError>>;
    /// Delete.
    fn delete<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a FileId,
    ) -> BoxFuture<'a, Result<StoredFile, StoreError>>;
    /// One page of files (id-cursor), optionally scope-filtered. Returns the
    /// page rows plus whether more remain. Pushed DB-side on the default store.
    fn list<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        filter: &'a FileFilter,
        cursor: &'a IdCursor,
    ) -> BoxFuture<'a, Result<(Vec<StoredFile>, bool), StoreError>>;
}

/// What an adapter implements to provide [`FilesStore`]; the methods are documented there.
pub trait FilesStoreImpl: Send + Sync {
    fn insert(&self, ws: &WorkspaceId, f: StoredFile) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn get(
        &self,
        ws: &WorkspaceId,
        id: &FileId,
    ) -> impl Future<Output = Result<StoredFile, StoreError>> + Send;
    fn content(
        &self,
        ws: &WorkspaceId,
        id: &FileId,
    ) -> impl Future<Output = Result<Bytes, StoreError>> + Send;
    fn delete(
        &self,
        ws: &WorkspaceId,
        id: &FileId,
    ) -> impl Future<Output = Result<StoredFile, StoreError>> + Send;
    fn list(
        &self,
        ws: &WorkspaceId,
        filter: &FileFilter,
        cursor: &IdCursor,
    ) -> impl Future<Output = Result<(Vec<StoredFile>, bool), StoreError>> + Send;
}

impl<T: FilesStoreImpl> FilesStore for T {
    fn insert<'a>(&'a self, ws: &'a WorkspaceId, f: StoredFile) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(FilesStoreImpl::insert(self, ws, f))
    }
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a FileId,
    ) -> BoxFuture<'a, Result<StoredFile, StoreError>> {
        Box::pin(FilesStoreImpl::get(self, ws, id))
    }
    fn content<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a FileId,
    ) -> BoxFuture<'a, Result<Bytes, StoreError>> {
        Box::pin(FilesStoreImpl::content(self, ws, id))
    }
    fn delete<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a FileId,
    ) -> BoxFuture<'a, Result<StoredFile, StoreError>> {
        Box::pin(FilesStoreImpl::delete(self, ws, id))
    }
    fn list<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        filter: &'a FileFilter,
        cursor: &'a IdCursor,
    ) -> BoxFuture<'a, Result<(Vec<StoredFile>, bool), StoreError>> {
        Box::pin(FilesStoreImpl::list(self, ws, filter, cursor))
    }
}
