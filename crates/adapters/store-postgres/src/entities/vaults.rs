//! [`VaultStore`] on Postgres via Diesel: the vault rows. The same store
//! implements [`::vaults::store::VaultCredentialStore`] in
//! [`super::vault_credentials`], so the vault aggregate keeps one repository.

use std::sync::Arc;

use ::vaults::store::{StoreError, VaultStore};
use ::vaults::types::{Vault, VaultId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::ports::secrets::SecretStore;
use shared::{Pagination, Timestamp, WorkspaceId};

use crate::schema::vaults;
use crate::{diesel_be, parse_doc, pool_be, to_doc, PgBackend};

mod archive;
mod secret;

/// Postgres + secret-store backed vaults store (Diesel).
#[derive(Clone)]
pub struct PgVaultsStore {
    pub(super) backend: PgBackend,
    secrets: Arc<dyn SecretStore>,
}

impl std::fmt::Debug for PgVaultsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgVaultsStore").finish_non_exhaustive()
    }
}

impl PgVaultsStore {
    /// Wrap a backend + secret store.
    pub fn new(backend: PgBackend, secrets: Arc<dyn SecretStore>) -> Self {
        Self { backend, secrets }
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = vaults)]
struct NewVault {
    workspace_id: String,
    vault_id: String,
    display_name: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl VaultStore for PgVaultsStore {
    async fn insert_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::insert_into(vaults::table)
            .values(NewVault {
                workspace_id: ws.as_str().to_owned(),
                vault_id: vault.id.as_str().to_owned(),
                display_name: vault.display_name.clone(),
                created_at: vault.created_at.as_datetime(),
                doc: to_doc(&vault)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = vaults::table
            .filter(vaults::workspace_id.eq(ws.as_str()))
            .filter(vaults::vault_id.eq(id.as_str()))
            .select(vaults::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::VaultNotFound)?)
    }

    async fn replace_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::update(
            vaults::table
                .filter(vaults::workspace_id.eq(ws.as_str()))
                .filter(vaults::vault_id.eq(vault.id.as_str())),
        )
        .set((vaults::display_name.eq(&vault.display_name), vaults::doc.eq(to_doc(&vault)?)))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::VaultNotFound);
        }
        Ok(())
    }

    async fn archive_vault(
        &self,
        ws: &WorkspaceId,
        id: &VaultId,
        at: Timestamp,
    ) -> Result<Vault, StoreError> {
        self.archive_cascade(ws, id, at).await
    }

    async fn delete_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        self.delete_cascade(ws, id).await
    }

    async fn list_vaults(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Vault>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = vaults::table
            .filter(vaults::workspace_id.eq(ws.as_str()))
            .order((vaults::created_at.desc(), vaults::vault_id.desc()))
            .select(vaults::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
