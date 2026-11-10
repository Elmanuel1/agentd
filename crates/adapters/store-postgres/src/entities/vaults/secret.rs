//! The secret side of [`PgVaultsStore`]: each credential's sealed material in
//! the [`shared::ports::secrets::SecretStore`], under its workspace, vault and
//! credential ids.

use ::vaults::store::StoreError;
use ::vaults::types::{CredentialAuthInput, CredentialId, VaultId};
use shared::ports::secrets::SecretKey;
use shared::WorkspaceId;

use diesel::dsl::sql;
use diesel::sql_types::Bool;
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;

use super::PgVaultsStore;
use crate::schema::credentials;
use crate::{diesel_be, pool_be, secret_be};

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
            Some(a @ CredentialAuthInput::EnvironmentVariable { .. }) => {
                self.secrets.put(&key.environment_variable(), &a.seal()).await.map_err(secret_be)
            }
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

    /// Where each `environment_variable` credential's secret lives, across
    /// every workspace: what `agentd vault-secrets tag` tags.
    pub async fn environment_variable_secrets(&self) -> Result<Vec<SecretKey>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<(String, String, String)> = credentials::table
            .filter(credentials::has_secret.eq(true))
            .filter(sql::<Bool>("doc->'auth'->>'type' = 'environment_variable'"))
            .select((credentials::workspace_id, credentials::vault_id, credentials::credential_id))
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        rows.iter()
            .map(|(ws, vault, id)| {
                Ok(SecretKey::new(ws, vault, id).map_err(secret_be)?.environment_variable())
            })
            .collect()
    }
}
