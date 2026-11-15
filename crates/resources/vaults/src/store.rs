//! Vault/credential persistence — workspace-scoped.
//!
//! The store holds credentials **with** their secret material (needed later
//! by the egress injector) as [`StoredCredential`]; only the secret-free
//! [`Credential`] projection ever leaves through the service.

use async_trait::async_trait;
use shared::{Pagination, Timestamp, WorkspaceId};

use crate::types::{Credential, CredentialAuthInput, CredentialId, Vault, VaultId};

/// A credential with its secret material — never serialized (contains
/// `SecretString`, which has no `Serialize` impl).
#[derive(Debug, Clone)]
pub struct StoredCredential {
    /// Wire projection.
    pub wire: Credential,
    /// Full auth incl. secrets — `None` after archive (secret purged).
    pub auth: Option<CredentialAuthInput>,
}

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such vault.
    #[error("vault not found")]
    VaultNotFound,
    /// No such credential.
    #[error("credential not found")]
    CredentialNotFound,
    /// The vault is archived.
    #[error("vault archived")]
    VaultArchived,
    /// The credential is archived.
    #[error("credential archived")]
    CredentialArchived,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port for vault rows.
#[async_trait]
pub trait VaultStore: Send + Sync {
    /// Insert a vault.
    async fn insert_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError>;
    /// Fetch a vault.
    async fn get_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError>;
    /// Replace a vault.
    async fn replace_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError>;
    /// The vaults of `ids` that exist in the workspace, in one read.
    async fn named_vaults(&self, ws: &WorkspaceId, ids: &[VaultId]) -> Result<Vec<Vault>, StoreError>;
    /// Archive a vault and its unarchived credentials, purging their secrets;
    /// an archived vault is answered as it is.
    async fn archive_vault(&self, ws: &WorkspaceId, id: &VaultId, at: Timestamp)
        -> Result<Vault, StoreError>;
    /// Delete a vault and its credentials, purging their secrets.
    async fn delete_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError>;
    /// All vaults, newest first.
    async fn list_vaults(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Vault>, StoreError>;
}

/// Persistence port for credential rows and their secrets, implemented by the
/// same store as [`VaultStore`]: the vault aggregate keeps one repository.
#[async_trait]
pub trait VaultCredentialStore: Send + Sync {
    /// Insert a credential into a vault.
    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError>;
    /// Fetch one credential.
    async fn get_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError>;
    /// Replace one credential.
    async fn replace_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError>;
    /// Delete one credential.
    async fn delete_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError>;
    /// All credentials in a vault.
    async fn list_credentials(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
    ) -> Result<Vec<StoredCredential>, StoreError>;
    /// The unarchived MCP credentials of `vaults` whose `mcp_server_url` is
    /// one of `urls`, in one read.
    async fn mcp_credentials(
        &self,
        ws: &WorkspaceId,
        vaults: &[String],
        urls: &[String],
    ) -> Result<Vec<Credential>, StoreError>;
    /// A credential's secret alone, with no row read.
    async fn credential_secret(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<CredentialAuthInput, StoreError>;
    /// The unarchived `environment_variable` credentials of `vaults`, oldest first.
    async fn environment_credentials(
        &self,
        ws: &WorkspaceId,
        vaults: &[String],
    ) -> Result<Vec<Credential>, StoreError>;
}
