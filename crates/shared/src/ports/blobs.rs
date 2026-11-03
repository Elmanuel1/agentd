//! Object storage for bytes (file contents, skill bundles, large memory
//! content). Bytes never live in the record database; the record store keeps
//! only metadata and the blob key.

use async_trait::async_trait;
use bytes::Bytes;

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
#[async_trait]
pub trait BlobStore: Send + Sync {
    /// Store bytes at `key` (overwrites).
    async fn put(&self, key: &str, bytes: Bytes) -> Result<(), BlobError>;
    /// Fetch bytes at `key`.
    async fn get(&self, key: &str) -> Result<Bytes, BlobError>;
    /// Delete `key` (idempotent).
    async fn delete(&self, key: &str) -> Result<(), BlobError>;
}
