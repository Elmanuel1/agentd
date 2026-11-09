//! [`VaultCredentialStore`] on [`PgVaultsStore`]: secret-free credential rows,
//! their secret material sealed to the secret store and never written to the
//! database.

use ::vaults::store::{StoreError, StoredCredential, VaultCredentialStore};
use ::vaults::types::{Credential, CredentialId, VaultId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use super::vaults::PgVaultsStore;
use crate::schema::{credentials, vaults};
use crate::{diesel_be, parse_doc, pool_be, to_doc};

impl PgVaultsStore {
    async fn require_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<String> = vaults::table
            .filter(vaults::workspace_id.eq(ws.as_str()))
            .filter(vaults::vault_id.eq(id.as_str()))
            .select(vaults::vault_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        exists.map(|_| ()).ok_or(StoreError::VaultNotFound)
    }

    /// The credential's wire object and whether it has a secret.
    async fn credential_row(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<(Credential, bool), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let row: Option<(serde_json::Value, bool)> = credentials::table
            .filter(credentials::workspace_id.eq(ws.as_str()))
            .filter(credentials::vault_id.eq(vault.as_str()))
            .filter(credentials::credential_id.eq(id.as_str()))
            .select((credentials::doc, credentials::has_secret))
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        let (doc, has_secret) = row.ok_or(StoreError::CredentialNotFound)?;
        Ok((parse_doc(doc)?, has_secret))
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = credentials)]
struct NewCredential {
    workspace_id: String,
    vault_id: String,
    credential_id: String,
    display_name: String,
    has_secret: bool,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl VaultCredentialStore for PgVaultsStore {
    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        self.require_vault(ws, vault).await?;
        let row = NewCredential {
            workspace_id: ws.as_str().to_owned(),
            vault_id: vault.as_str().to_owned(),
            credential_id: cred.wire.id.as_str().to_owned(),
            display_name: cred.wire.display_name.clone(),
            has_secret: cred.auth.is_some(),
            created_at: cred.wire.created_at.as_datetime(),
            doc: to_doc(&cred.wire)?,
        };
        self.put_secret(ws, vault, &cred.wire.id, &cred.auth).await?;
        // The connection is taken after the secret write and released before
        // any cleanup, so no call to the secret store holds one.
        let inserted: Result<usize, StoreError> = async {
            let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
            diesel::insert_into(credentials::table).values(row).execute(&mut conn).await.map_err(diesel_be)
        }
        .await;
        if inserted.is_err() {
            // No row will ever point at the secret just written: take it back out.
            self.put_secret(ws, vault, &cred.wire.id, &None).await?;
        }
        inserted.map(|_| ())
    }

    async fn get_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        let (wire, has_secret) = self.credential_row(ws, vault, id).await?;
        let auth = if has_secret { Some(self.read_secret(ws, vault, id).await?) } else { None };
        Ok(StoredCredential { wire, auth })
    }

    async fn replace_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        let affected = diesel::update(
            credentials::table
                .filter(credentials::workspace_id.eq(ws.as_str()))
                .filter(credentials::vault_id.eq(vault.as_str()))
                .filter(credentials::credential_id.eq(cred.wire.id.as_str())),
        )
        .set((
            credentials::display_name.eq(&cred.wire.display_name),
            credentials::has_secret.eq(cred.auth.is_some()),
            credentials::doc.eq(to_doc(&cred.wire)?),
        ))
        .execute(&mut *self.backend.diesel.write().await.map_err(pool_be)?)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::CredentialNotFound);
        }
        self.put_secret(ws, vault, &cred.wire.id, &cred.auth).await
    }

    async fn delete_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        // The secret is not read: a credential whose secret is already gone
        // must still be deletable, and the caller returns only the wire object.
        let (wire, _) = self.credential_row(ws, vault, id).await?;
        diesel::delete(
            credentials::table
                .filter(credentials::workspace_id.eq(ws.as_str()))
                .filter(credentials::vault_id.eq(vault.as_str()))
                .filter(credentials::credential_id.eq(id.as_str())),
        )
        .execute(&mut *self.backend.diesel.write().await.map_err(pool_be)?)
        .await
        .map_err(diesel_be)?;
        self.put_secret(ws, vault, id, &None).await?;
        Ok(StoredCredential { wire, auth: None })
    }

    async fn list_credentials(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
    ) -> Result<Vec<StoredCredential>, StoreError> {
        self.require_vault(ws, vault).await?;
        // Secret-free listing: `auth` is fetched per-credential on demand.
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = credentials::table
            .filter(credentials::workspace_id.eq(ws.as_str()))
            .filter(credentials::vault_id.eq(vault.as_str()))
            .order(credentials::created_at.asc())
            .select(credentials::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(|d| Ok(StoredCredential { wire: parse_doc(d)?, auth: None })).collect()
    }
}
