//! Object storage for bytes (file contents, skill bundles, large memory
//! content). Bytes never live in the record database; the record store keeps
//! only metadata and the blob key.

use crate::Timestamp;
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
    /// One page of the objects whose key starts with `prefix`, from where `after`, a page's `next`, left off.
    fn list<'a>(
        &'a self,
        prefix: &'a str,
        after: Option<String>,
    ) -> BoxFuture<'a, Result<Page<String>, BlobError>>;
}

/// One page of a store's listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<K> {
    /// What the page holds.
    pub items: Vec<Stored<K>>,
    /// Where the next page starts; none after the last.
    pub next: Option<String>,
}

/// An object or secret a store holds, and when it was last written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored<K> {
    /// Its key.
    pub key: K,
    /// When it was last written.
    pub written_at: Timestamp,
}

/// What an adapter implements to provide [`BlobStore`]; the methods are documented there.
pub trait NativeBlobStore: Send + Sync {
    fn put(&self, key: &str, bytes: Bytes) -> impl Future<Output = Result<(), BlobError>> + Send;
    fn get(&self, key: &str) -> impl Future<Output = Result<Bytes, BlobError>> + Send;
    fn delete(&self, key: &str) -> impl Future<Output = Result<(), BlobError>> + Send;
    fn list(
        &self,
        prefix: &str,
        after: Option<String>,
    ) -> impl Future<Output = Result<Page<String>, BlobError>> + Send;
}

impl<T: NativeBlobStore> BlobStore for T {
    fn put<'a>(&'a self, key: &'a str, bytes: Bytes) -> BoxFuture<'a, Result<(), BlobError>> {
        Box::pin(NativeBlobStore::put(self, key, bytes))
    }
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<Bytes, BlobError>> {
        Box::pin(NativeBlobStore::get(self, key))
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<(), BlobError>> {
        Box::pin(NativeBlobStore::delete(self, key))
    }
    fn list<'a>(
        &'a self,
        prefix: &'a str,
        after: Option<String>,
    ) -> BoxFuture<'a, Result<Page<String>, BlobError>> {
        Box::pin(NativeBlobStore::list(self, prefix, after))
    }
}
