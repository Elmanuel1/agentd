//! Vault/credential persistence — workspace-scoped.
//!
//! The store holds credentials **with** their secret material (needed later
//! by the egress injector) as [`StoredCredential`]; only the secret-free
//! [`Credential`] projection ever leaves through the service.

use async_trait::async_trait;
use shared::{Pagination, Timestamp, WorkspaceId};
use webhooks::Delivery;

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

/// How a refresh's final write landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimedWrite {
    /// The row still held the claim: the new token and expiry are stored.
    Matched,
    /// An update came between the claim and the write: the row is the
    /// update's, the secret the refresh's.
    Changed,
    /// The credential was archived or deleted meanwhile; its secret is out.
    Gone,
}

/// The deliveries a vault's archive sends, from the ids of the credentials it
/// archives.
pub type CascadeDeliveries<'a> = dyn Fn(&[String]) -> Vec<Delivery> + Send + Sync + 'a;

/// Persistence port for vault rows.
#[async_trait]
pub trait VaultStore: VaultCascade {
    /// Its webhook deliveries are written in the same transaction.
    async fn insert_vault(
        &self,
        ws: &WorkspaceId,
        vault: Vault,
        deliveries: &[Delivery],
    ) -> Result<(), StoreError>;
    /// Fetch a vault.
    async fn get_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError>;
    /// Replace a vault.
    async fn replace_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError>;
    /// The vaults of `ids` that exist in the workspace, in one read.
    async fn named_vaults(&self, ws: &WorkspaceId, ids: &[VaultId]) -> Result<Vec<Vault>, StoreError>;
    /// All vaults, newest first.
    async fn list_vaults(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Vault>, StoreError>;
}

/// Persistence port for credential rows and their secrets, implemented by the
/// same store as [`VaultStore`]: the vault aggregate keeps one repository.
#[async_trait]
pub trait VaultCredentialStore: CredentialReads + CredentialClaims + CredentialWrites {
    /// Fetch one credential.
    async fn get_credential(
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
}

/// A vault's archive and delete, each taking its credentials with it.
#[async_trait]
pub trait VaultCascade: Send + Sync {
    /// Archive a vault and its unarchived credentials, purging their secrets,
    /// with the `deliveries` built from the archived credentials' ids;
    /// an archived vault is answered as it is.
    async fn archive_vault(
        &self,
        ws: &WorkspaceId,
        id: &VaultId,
        at: Timestamp,
        deliveries: &CascadeDeliveries<'_>,
    ) -> Result<Vault, StoreError>;
    /// Delete a vault and its credentials, purging their secrets, with `deliveries`.
    async fn delete_vault(
        &self,
        ws: &WorkspaceId,
        id: &VaultId,
        deliveries: &[Delivery],
    ) -> Result<Vault, StoreError>;
}

/// The reads of credentials across a session's vaults, and of one secret.
#[async_trait]
pub trait CredentialReads: Send + Sync {
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

/// A credential write claimed on the `updated_at` the writer read.
#[async_trait]
pub trait CredentialClaims: Send + Sync {
    /// Move the unarchived row's `updated_at` from `read` to `now`; `false`
    /// when it no longer reads `read`.
    async fn claim_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        read: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError>;
    /// Store the secret, then the row while its `updated_at` is still
    /// `claimed`.
    async fn write_claimed(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: &StoredCredential,
        claimed: Timestamp,
    ) -> Result<ClaimedWrite, StoreError>;
}

/// A credential's create, archive and delete, with their webhook deliveries.
#[async_trait]
pub trait CredentialWrites: Send + Sync {
    /// Insert a credential into a vault.
    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
        deliveries: &[Delivery],
    ) -> Result<(), StoreError>;
    /// Archive one credential: `cred` with its archive stamp and no secret.
    async fn archive_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
        deliveries: &[Delivery],
    ) -> Result<(), StoreError>;
    /// `deliveries` are written only when a row was deleted.
    async fn delete_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        deliveries: &[Delivery],
    ) -> Result<StoredCredential, StoreError>;
    /// For a change with no row to write beside its deliveries.
    async fn insert_deliveries(&self, deliveries: &[Delivery]) -> Result<(), StoreError>;
}
