//! secrets-blob: a [`SecretStore`] over a [`BlobStore`]: each secret is one
//! object, `secrets/<workspace>/<vault>/<credential>`, stored as written. For
//! development only: the material is not encrypted, and agentd boots with it
//! only when `AGENTD_DEV_SECRETS=1`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::{ExposeSecret, SecretError, SecretKey, SecretStore, SecretString};

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

fn object_key(key: &SecretKey) -> String {
    format!("secrets/{key}")
}

fn secret_error(e: BlobError) -> SecretError {
    match e {
        BlobError::NotFound(key) => SecretError::NotFound(key),
        BlobError::Backend(m) => SecretError::Backend(m),
    }
}

#[async_trait]
impl SecretStore for BlobSecretStore {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        let bytes = Bytes::copy_from_slice(secret.expose_secret().as_bytes());
        self.blobs.put(&object_key(key), bytes).await.map_err(secret_error)
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        let bytes = self.blobs.get(&object_key(key)).await.map_err(secret_error)?;
        std::str::from_utf8(&bytes)
            .map(SecretString::from)
            .map_err(|e| SecretError::Backend(format!("secret at {key} is not UTF-8: {e}")))
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        match self.blobs.delete(&object_key(key)).await {
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
        let key = SecretKey::new("wrkspc_a", "vlt_a", "cred_a").unwrap();
        secrets.put(&key, &SecretString::from("{\"token\":\"t\"}")).await.unwrap();
        assert_eq!(secrets.get(&key).await.unwrap().expose_secret(), "{\"token\":\"t\"}");
        assert!(blobs.0.lock().unwrap().contains_key("secrets/wrkspc_a/vlt_a/cred_a"));
        secrets.delete(&key).await.unwrap();
        secrets.delete(&key).await.unwrap();
        assert!(matches!(secrets.get(&key).await, Err(SecretError::NotFound(_))));
    }

    #[tokio::test]
    async fn unreadable_material_is_an_error() {
        let blobs = Arc::new(MemoryBlobs::default());
        blobs.put("secrets/wrkspc_a/vlt_a/cred_bad", Bytes::from_static(&[0xff, 0xfe])).await.unwrap();
        let secrets = BlobSecretStore::new(blobs);
        let key = SecretKey::new("wrkspc_a", "vlt_a", "cred_bad").unwrap();
        assert!(matches!(secrets.get(&key).await, Err(SecretError::Backend(_))));
    }
}
