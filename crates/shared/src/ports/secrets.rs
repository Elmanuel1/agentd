//! Secret material (vault credentials), stored apart from the record database.
//!
//! Values cross this port as [`SecretString`]: its `Debug` is redacted and its
//! memory is zeroed on drop, so a value cannot reach a log through a
//! formatter.

use async_trait::async_trait;
pub use secrecy::{ExposeSecret, SecretString};

use crate::charset::is_id_charset;

/// Secret-store failure. No variant ever carries secret material.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No secret at the key.
    #[error("secret not found: {0}")]
    NotFound(String),
    /// A key part outside the id charset; nothing was called.
    #[error("invalid secret key: {0}")]
    InvalidKey(String),
    /// Backend/transport failure, or stored material that cannot be read.
    #[error("secret backend: {0}")]
    Backend(String),
}

/// Where one credential's secret lives. Each part is in the id charset
/// ([`is_id_charset`]), checked when the key is made, so a key can never
/// name a path outside its own workspace and vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretKey {
    workspace: String,
    vault: String,
    credential: String,
}

impl SecretKey {
    /// The key of `credential` in `vault` of `workspace`.
    pub fn new(workspace: &str, vault: &str, credential: &str) -> Result<Self, SecretError> {
        for (part, value) in [("workspace", workspace), ("vault", vault), ("credential", credential)] {
            if !is_id_charset(value) {
                return Err(SecretError::InvalidKey(format!("the {part} id is outside the id charset")));
            }
        }
        Ok(Self {
            workspace: workspace.to_owned(),
            vault: vault.to_owned(),
            credential: credential.to_owned(),
        })
    }

    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    pub fn vault(&self) -> &str {
        &self.vault
    }

    pub fn credential(&self) -> &str {
        &self.credential
    }
}

/// `workspace/vault/credential`.
impl std::fmt::Display for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.workspace, self.vault, self.credential)
    }
}

/// Secret storage by key.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Store `secret` at `key` (overwrites).
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError>;
    /// Fetch the secret at `key`.
    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError>;
    /// Delete `key`; a missing key is not an error.
    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_takes_only_id_charset_parts() {
        let key = SecretKey::new("wrkspc_01a", "vlt_01b", "cred_01c").unwrap();
        assert_eq!(key.to_string(), "wrkspc_01a/vlt_01b/cred_01c");
        for bad in ["", "a/b", "..", "a.b", "*", "a-b", "a b", "é", "x\n", &"a".repeat(65)] {
            for parts in [[bad, "v", "c"], ["w", bad, "c"], ["w", "v", bad]] {
                let err = SecretKey::new(parts[0], parts[1], parts[2]).unwrap_err();
                assert!(matches!(err, SecretError::InvalidKey(_)), "{parts:?}");
                assert!(bad.is_empty() || !err.to_string().contains(bad), "the error echoes input");
            }
        }
    }
}
