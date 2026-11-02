//! secrets-blob: a [`SecretStore`] over a [`BlobStore`]: each secret is one
//! object, stored as written. A stand-in for a secret manager in development;
//! the material is not encrypted.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{SecretError, SecretStore};

/// Secrets kept as objects in a blob store.
#[derive(Clone)]
pub struct BlobSecretStore {
    blobs: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for BlobSecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobSecretStore").finish_non_exhaustive()
    }
}

impl BlobSecretStore {
    /// Keep secrets in `blobs`.
    pub fn new(blobs: Arc<dyn BlobStore>) -> Self {
        Self { blobs }
    }
}

fn secret_error(e: BlobError) -> SecretError {
    match e {
        BlobError::NotFound(key) => SecretError::NotFound(key),
        BlobError::Backend(m) => SecretError::Backend(m),
    }
}

#[async_trait]
impl SecretStore for BlobSecretStore {
    async fn put(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.blobs.put(key, Bytes::from(secret.to_owned())).await.map_err(secret_error)
    }

    async fn get(&self, key: &str) -> Result<String, SecretError> {
        let bytes = self.blobs.get(key).await.map_err(secret_error)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|e| SecretError::Backend(format!("secret at {key} is not UTF-8: {e}")))
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        match self.blobs.delete(key).await {
            Ok(()) | Err(BlobError::NotFound(_)) => Ok(()),
            Err(e) => Err(secret_error(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct MemoryBlobs(Mutex<BTreeMap<String, Bytes>>);

    #[async_trait]
    impl BlobStore for MemoryBlobs {
        async fn put(&self, key: &str, bytes: Bytes) -> Result<(), BlobError> {
            self.0.lock().unwrap().insert(key.to_owned(), bytes);
            Ok(())
        }
        async fn get(&self, key: &str) -> Result<Bytes, BlobError> {
            self.0.lock().unwrap().get(key).cloned().ok_or_else(|| BlobError::NotFound(key.to_owned()))
        }
        async fn delete(&self, key: &str) -> Result<(), BlobError> {
            self.0.lock().unwrap().remove(key).map(|_| ()).ok_or_else(|| BlobError::NotFound(key.to_owned()))
        }
    }

    #[tokio::test]
    async fn secrets_round_trip_and_delete_is_idempotent() {
        let blobs = Arc::new(MemoryBlobs::default());
        let secrets = BlobSecretStore::new(blobs.clone());
        secrets.put("secrets/a", "{\"token\":\"t\"}").await.unwrap();
        assert_eq!(secrets.get("secrets/a").await.unwrap(), "{\"token\":\"t\"}");
        secrets.delete("secrets/a").await.unwrap();
        secrets.delete("secrets/a").await.unwrap();
        assert!(matches!(secrets.get("secrets/a").await, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn unreadable_material_is_an_error() {
        let blobs = Arc::new(MemoryBlobs::default());
        blobs.put("secrets/bad", Bytes::from_static(&[0xff, 0xfe])).await.unwrap();
        let secrets = BlobSecretStore::new(blobs);
        assert!(matches!(secrets.get("secrets/bad").await, Err(SecretError::Backend(_))));
    }
}
