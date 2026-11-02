//! `VaultsStore` on Postgres via Diesel — vault + secret-free credential rows;
//! the secret material is sealed to object storage (a secret-manager stand-in),
//! keyed by credential id, and NEVER written to the database.

use std::sync::Arc;

use ::vaults::store::{StoreError, StoredCredential, VaultsStore};
use ::vaults::types::{Credential, CredentialAuthInput, CredentialId, Vault, VaultId};
use async_trait::async_trait;
use blobs_s3::BlobStore;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use crate::schema::{credentials, vaults};
use crate::{blob_be, diesel_be, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres + secret-store backed vaults store (Diesel).
#[derive(Clone)]
pub struct PgVaultsStore {
    backend: PgBackend,
    secrets: Arc<dyn BlobStore>,
}

impl std::fmt::Debug for PgVaultsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgVaultsStore").finish_non_exhaustive()
    }
}

impl PgVaultsStore {
    /// Wrap a backend + secret store.
    pub fn new(backend: PgBackend, secrets: Arc<dyn BlobStore>) -> Self {
        Self { backend, secrets }
    }

    fn secret_key(ws: &WorkspaceId, vault: &VaultId, id: &CredentialId) -> String {
        format!("secrets/{}/{}/{}", ws.as_str(), vault.as_str(), id.as_str())
    }

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

    async fn put_secret(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        auth: &Option<CredentialAuthInput>,
    ) -> Result<(), StoreError> {
        let key = Self::secret_key(ws, vault, id);
        match auth {
            Some(a) => self.secrets.put(&key, Bytes::from(a.seal().into_bytes())).await.map_err(blob_be),
            None => match self.secrets.delete(&key).await {
                Ok(()) | Err(blobs_s3::BlobError::NotFound(_)) => Ok(()),
                Err(e) => Err(blob_be(e)),
            },
        }
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
impl VaultsStore for PgVaultsStore {
    async fn insert_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
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
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
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

    async fn delete_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        // Propagate a listing failure — swallowing it would skip secret cleanup
        // and orphan the sealed material in the object store.
        let creds = self.list_credentials(ws, id).await?;
        let vault = self.get_vault(ws, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::delete(
            credentials::table
                .filter(credentials::workspace_id.eq(ws.as_str()))
                .filter(credentials::vault_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        diesel::delete(
            vaults::table
                .filter(vaults::workspace_id.eq(ws.as_str()))
                .filter(vaults::vault_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        for c in creds {
            self.put_secret(ws, id, &c.wire.id, &None).await?;
        }
        Ok(vault)
    }

    async fn list_vaults(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Vault>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = vaults::table
            .filter(vaults::workspace_id.eq(ws.as_str()))
            .order(vaults::created_at.asc())
            .select(vaults::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        self.require_vault(ws, vault).await?;
        self.put_secret(ws, vault, &cred.wire.id, &cred.auth).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(credentials::table)
            .values(NewCredential {
                workspace_id: ws.as_str().to_owned(),
                vault_id: vault.as_str().to_owned(),
                credential_id: cred.wire.id.as_str().to_owned(),
                display_name: cred.wire.display_name.clone(),
                has_secret: cred.auth.is_some(),
                created_at: cred.wire.created_at.as_datetime(),
                doc: to_doc(&cred.wire)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
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
        let wire: Credential = parse_doc(doc)?;
        let auth = if has_secret {
            let sealed = self.secrets.get(&Self::secret_key(ws, vault, id)).await.map_err(blob_be)?;
            CredentialAuthInput::unseal(&String::from_utf8_lossy(&sealed))
        } else {
            None
        };
        Ok(StoredCredential { wire, auth })
    }

    async fn replace_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
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
        .execute(&mut conn)
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
        let existing = self.get_credential(ws, vault, id).await?;
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::delete(
            credentials::table
                .filter(credentials::workspace_id.eq(ws.as_str()))
                .filter(credentials::vault_id.eq(vault.as_str()))
                .filter(credentials::credential_id.eq(id.as_str())),
        )
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        self.put_secret(ws, vault, id, &None).await?;
        Ok(existing)
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
