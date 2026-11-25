use bytes::Bytes;
use shared::ports::blobs::{BlobError, NativeBlobStore, Stored};

/// A blob store whose reads and writes fail with not-found and whose deletes succeed.
pub(super) struct Failing;

impl NativeBlobStore for Failing {
    async fn put(&self, key: &str, _bytes: Bytes) -> Result<(), BlobError> {
        Err(BlobError::NotFound(key.to_owned()))
    }
    async fn get(&self, key: &str) -> Result<Bytes, BlobError> {
        Err(BlobError::NotFound(key.to_owned()))
    }
    async fn delete(&self, _key: &str) -> Result<(), BlobError> {
        Ok(())
    }
    async fn list(&self, _prefix: &str) -> Result<Vec<Stored<String>>, BlobError> {
        Ok(vec![])
    }
}
