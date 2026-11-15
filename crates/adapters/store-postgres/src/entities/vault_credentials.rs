//! [`VaultCredentialStore`] on [`PgVaultsStore`]: secret-free credential rows,
//! their secret material sealed to the secret store and never written to the
//! database.

use ::vaults::store::{RefreshWrite, StoreError, StoredCredential, VaultCredentialStore};
use ::vaults::types::{Credential, CredentialAuthInput, CredentialId, VaultId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::dsl::sql;
use diesel::sql_types::Bool;
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{OrgId, Timestamp, WorkspaceId};
use webhooks::EventKind;

use super::vaults::PgVaultsStore;
use crate::schema::{credentials, vaults};
use crate::{diesel_be, parse_doc, pool_be};

mod mcp;
mod refresh;
mod write;

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
        org: &OrgId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        self.insert_locked(ws, org, vault, cred).await
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
        self.replace_guarded(ws, vault, cred, &[]).await
    }

    async fn archive_credential(
        &self,
        ws: &WorkspaceId,
        org: &OrgId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        let kind = EventKind::CredentialArchived { vault_id: vault.as_str().to_owned() };
        let facts = self.facts(org, ws, vec![(kind, cred.wire.id.as_str().to_owned())]);
        self.replace_guarded(ws, vault, cred, &facts).await
    }

    async fn delete_credential(
        &self,
        ws: &WorkspaceId,
        org: &OrgId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        self.delete_row(ws, org, vault, id).await
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
            .order((credentials::created_at.desc(), credentials::credential_id.desc()))
            .select(credentials::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(|d| Ok(StoredCredential { wire: parse_doc(d)?, auth: None })).collect()
    }

    async fn mcp_credentials(
        &self,
        ws: &WorkspaceId,
        vaults: &[String],
        urls: &[String],
    ) -> Result<Vec<Credential>, StoreError> {
        self.mcp_rows(ws, vaults, urls).await
    }

    async fn credential_secret(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<CredentialAuthInput, StoreError> {
        self.read_secret(ws, vault, id).await
    }

    async fn claim_refresh(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        read: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        self.claim_refresh_row(ws, vault, id, read, now).await
    }

    async fn finish_refresh(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: &StoredCredential,
        claimed: Timestamp,
    ) -> Result<RefreshWrite, StoreError> {
        self.finish_refresh_row(ws, vault, cred, claimed).await
    }

    async fn refresh_failed(
        &self,
        ws: &WorkspaceId,
        org: &OrgId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<(), StoreError> {
        let kind = EventKind::CredentialRefreshFailed { vault_id: vault.as_str().to_owned() };
        let facts = self.facts(org, ws, vec![(kind, id.as_str().to_owned())]);
        super::webhook_deliveries::publish(&self.backend, &facts).await.map_err(StoreError::Backend)
    }

    async fn environment_credentials(
        &self,
        ws: &WorkspaceId,
        vaults: &[String],
    ) -> Result<Vec<Credential>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = credentials::table
            .filter(credentials::workspace_id.eq(ws.as_str()))
            .filter(credentials::vault_id.eq_any(vaults))
            .filter(sql::<Bool>(
                "doc->'auth'->>'type' = 'environment_variable' AND doc->>'archived_at' IS NULL",
            ))
            .order(credentials::created_at.asc())
            .select(credentials::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
