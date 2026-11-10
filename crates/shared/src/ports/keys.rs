//! API keys: where the caller's key is resolved to its tenant. Workspace keys,
//! admin keys and environment keys are separate stores; the route a request
//! asks for, and the key's prefix, decide which one is asked.

pub mod material;

use async_trait::async_trait;

use crate::{AdminContext, AuthContext, EnvironmentCaller};

/// The key store could not be read. It never carries the key.
#[derive(Debug, thiserror::Error)]
#[error("key store: {0}")]
pub struct KeyError(pub String);

/// Resolves an API key to its tenant.
#[async_trait]
pub trait KeySource: Send + Sync {
    /// The tenant of `key`; `None` for a key that is unknown or revoked.
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError>;
}

/// Resolves an admin key to its organization.
#[async_trait]
pub trait AdminKeySource: Send + Sync {
    /// The organization of `key`; `None` for a key that is unknown or revoked.
    async fn resolve(&self, key: &str) -> Result<Option<AdminContext>, KeyError>;
}

/// What an environment key grants: one self-hosted environment's queue in
/// its workspace, or agentd's runtime queue, whose items span workspaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentGrant {
    /// A self-hosted environment's key.
    Environment(AuthContext, EnvironmentCaller),
    /// agentd's runtime key.
    Runtime,
}

/// Resolves an environment key to what it grants.
#[async_trait]
pub trait EnvironmentKeySource: Send + Sync {
    /// What `key` grants; `None` for a key that is unknown or revoked.
    async fn resolve(&self, key: &str) -> Result<Option<EnvironmentGrant>, KeyError>;
}

/// The server-side secret every stored API key digest is keyed with. Its
/// `Debug` is redacted and its memory is zeroed on drop.
pub struct KeyPepper(secrecy::SecretBox<[u8; 32]>);

impl KeyPepper {
    /// Hold `bytes`, which the caller zeroes.
    pub fn new(bytes: &[u8; 32]) -> Self {
        Self(secrecy::SecretBox::new(Box::new(*bytes)))
    }

    /// The pepper, to key a digest with. Never log it.
    pub fn expose(&self) -> &[u8; 32] {
        secrecy::ExposeSecret::expose_secret(&self.0)
    }
}

impl std::fmt::Debug for KeyPepper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyPepper(***)")
    }
}
