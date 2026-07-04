//! Vault/credential persistence — workspace-scoped.
//!
//! The store holds credentials **with** their secret material (needed later
//! by the egress injector) as [`StoredCredential`]; only the secret-free
//! [`Credential`] projection ever leaves through the service.

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use parking_lot::RwLock;

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
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(String),
}

/// Persistence port.
#[async_trait]
pub trait VaultsStore: Send + Sync {
    /// Insert a vault.
    async fn insert_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError>;
    /// Fetch a vault.
    async fn get_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError>;
    /// Replace a vault.
    async fn replace_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError>;
    /// Delete a vault (and its credentials).
    async fn delete_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError>;
    /// All vaults, insertion order.
    async fn list_vaults(&self, ws: &WorkspaceId) -> Result<Vec<Vault>, StoreError>;

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
}

type VaultEntry = (Vault, BTreeMap<CredentialId, StoredCredential>);

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemoryVaultsStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<VaultId, VaultEntry>>>,
}

impl InMemoryVaultsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl VaultsStore for InMemoryVaultsStore {
    async fn insert_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(vault.id.clone(), (vault, BTreeMap::new()));
        Ok(())
    }

    async fn get_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(v, _)| v.clone())
            .ok_or(StoreError::VaultNotFound)
    }

    async fn replace_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&vault.id))
            .ok_or(StoreError::VaultNotFound)?;
        entry.0 = vault;
        Ok(())
    }

    async fn delete_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        self.inner
            .write()
            .get_mut(ws)
            .and_then(|m| m.remove(id))
            .map(|(v, _)| v)
            .ok_or(StoreError::VaultNotFound)
    }

    async fn list_vaults(&self, ws: &WorkspaceId) -> Result<Vec<Vault>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| m.values().map(|(v, _)| v.clone()).collect())
            .unwrap_or_default())
    }

    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(vault))
            .ok_or(StoreError::VaultNotFound)?;
        entry.1.insert(cred.wire.id.clone(), cred);
        Ok(())
    }

    async fn get_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(vault))
            .ok_or(StoreError::VaultNotFound)?;
        entry
            .1
            .get(id)
            .cloned()
            .ok_or(StoreError::CredentialNotFound)
    }

    async fn replace_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(vault))
            .ok_or(StoreError::VaultNotFound)?;
        if !entry.1.contains_key(&cred.wire.id) {
            return Err(StoreError::CredentialNotFound);
        }
        entry.1.insert(cred.wire.id.clone(), cred);
        Ok(())
    }

    async fn delete_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        let mut g = self.inner.write();
        let entry = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(vault))
            .ok_or(StoreError::VaultNotFound)?;
        entry.1.remove(id).ok_or(StoreError::CredentialNotFound)
    }

    async fn list_credentials(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
    ) -> Result<Vec<StoredCredential>, StoreError> {
        let g = self.inner.read();
        let entry = g
            .get(ws)
            .and_then(|m| m.get(vault))
            .ok_or(StoreError::VaultNotFound)?;
        Ok(entry.1.values().cloned().collect())
    }
}
