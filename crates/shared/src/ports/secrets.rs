//! Secret material (vault credentials), stored apart from the record database.

use async_trait::async_trait;

/// Secret-store failure.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No secret at the key.
    #[error("secret not found: {0}")]
    NotFound(String),
    /// Backend/transport failure, or stored material that cannot be read.
    #[error("secret backend: {0}")]
    Backend(String),
}

/// Secret storage by key.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Store `secret` at `key` (overwrites).
    async fn put(&self, key: &str, secret: &str) -> Result<(), SecretError>;
    /// Fetch the secret at `key`.
    async fn get(&self, key: &str) -> Result<String, SecretError>;
    /// Delete `key`; a missing key is not an error.
    async fn delete(&self, key: &str) -> Result<(), SecretError>;
}
