//! The secret side of [`PgVaultsStore`]: each credential's sealed material in
//! the [`shared::ports::secrets::SecretStore`], under its workspace, vault and
//! credential ids.

use ::vaults::store::StoreError;
use ::vaults::types::{CredentialAuthInput, CredentialId, VaultId};
use shared::ports::secrets::SecretKey;
use shared::WorkspaceId;

use super::PgVaultsStore;
use crate::secret_be;

impl PgVaultsStore {
    fn secret_key(ws: &WorkspaceId, vault: &VaultId, id: &CredentialId) -> Result<SecretKey, StoreError> {
        SecretKey::new(ws.as_str(), vault.as_str(), id.as_str()).map_err(secret_be)
    }

    /// Seal `auth` as the credential's secret, or delete the secret when `None`.
    pub(in crate::entities) async fn put_secret(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        auth: &Option<CredentialAuthInput>,
    ) -> Result<(), StoreError> {
        let key = Self::secret_key(ws, vault, id)?;
        match auth {
            Some(a) => self.secrets.put(&key, &a.seal()).await.map_err(secret_be),
            None => self.secrets.delete(&key).await.map_err(secret_be),
        }
    }

    pub(in crate::entities) async fn read_secret(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<CredentialAuthInput, StoreError> {
        let sealed = self.secrets.get(&Self::secret_key(ws, vault, id)?).await.map_err(secret_be)?;
        CredentialAuthInput::unseal(&sealed).map_err(secret_be)
    }
}
