//! Credentials minted on demand instead of configured: a cloud IAM token that
//! is a Postgres password, or a Kafka OAUTHBEARER token.

use crate::BackendError;
use crate::ports::BoxFuture;

/// A password minted for each new database connection.
pub trait PasswordSource: Send + Sync + std::fmt::Debug {
    /// A password for `user` at `host:port`, valid for at least one login.
    fn password<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        user: &'a str,
    ) -> BoxFuture<'a, Result<String, BackendError>>;
}

/// An OAUTHBEARER token and when it expires.
#[derive(Clone)]
pub struct BearerToken {
    /// The token itself.
    pub value: String,
    /// The principal the token names.
    pub principal: String,
    /// Expiry, in milliseconds since the Unix epoch.
    pub expires_at_ms: i64,
}

/// Shows the principal and expiry, never the token.
impl std::fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BearerToken")
            .field("principal", &self.principal)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish_non_exhaustive()
    }
}

/// OAUTHBEARER tokens, asked for again before each one expires.
pub trait BearerTokenSource: Send + Sync + std::fmt::Debug {
    /// A fresh token.
    fn token(&self) -> BoxFuture<'_, Result<BearerToken, BackendError>>;
}
