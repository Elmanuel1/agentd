//! API keys: where the caller's key is resolved to its tenant.

use async_trait::async_trait;

use crate::AuthContext;

/// Resolves an API key to its tenant.
#[async_trait]
pub trait KeySource: Send + Sync {
    /// The tenant of `key`, or `None` for an unknown key.
    async fn resolve(&self, key: &str) -> Option<AuthContext>;
}
