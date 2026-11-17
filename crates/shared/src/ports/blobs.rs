//! Object storage for bytes (file contents, skill bundles, large memory
//! content). Bytes never live in the record database; the record store keeps
//! only metadata and the blob key.

use crate::ports::BoxFuture;
use bytes::Bytes;
use std::future::Future;

/// Blob-store failure.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// The object does not exist.
    #[error("blob not found: {0}")]
    NotFound(String),
    /// Backend/transport failure.
    #[error("blob backend: {0}")]
    Backend(String),
}

/// Object storage: put/get/delete by key, within one bucket.
pub trait BlobStore: Send + Sync {
    /// Store bytes at `key` (overwrites).
    fn put<'a>(&'a self, key: &'a str, bytes: Bytes) -> BoxFuture<'a, Result<(), BlobError>>;
    /// Fetch bytes at `key`.
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Bytes, BlobError>>;
    /// Delete `key` (idempotent).
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), BlobError>>;
}

/// What an adapter implements to provide [`BlobStore`]; the methods are documented there.
pub trait BlobStoreImpl: Send + Sync {
    fn put(&self, key: &str, bytes: Bytes) -> impl Future<Output = Result<(), BlobError>> + Send;
    fn get(&self, key: &str) -> impl Future<Output = Result<Bytes, BlobError>> + Send;
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), BlobError>> + Send;
}

impl<T: BlobStoreImpl> BlobStore for T {
    fn put<'a>(&'a self, key: &'a str, bytes: Bytes) -> BoxFuture<'a, Result<(), BlobError>> {
        Box::pin(BlobStoreImpl::put(self, key, bytes))
    }
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Bytes, BlobError>> {
        Box::pin(BlobStoreImpl::get(self, key))
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), BlobError>> {
        Box::pin(BlobStoreImpl::delete(self, key))
    }
}
