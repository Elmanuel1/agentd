//! `VaultsStore` on Postgres — vault + secret-free credential rows; the secret
//! material is sealed to object storage (a secret-manager stand-in), keyed by
//! credential id, and NEVER written to the database.

use std::sync::Arc;

use agentd_blobs::BlobStore;
use agentd_core::WorkspaceId;
use agentd_vaults::store::{StoreError, StoredCredential, VaultsStore};
use agentd_vaults::types::{Credential, CredentialAuthInput, CredentialId, Vault, VaultId};
use async_trait::async_trait;
use bytes::Bytes;
use sqlx::types::Json;

use crate::{sqlx_msg, PgBackend};

/// Postgres + secret-store backed vaults store.
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
    /// Wrap a pool + secret store.
    pub fn new(backend: PgBackend, secrets: Arc<dyn BlobStore>) -> Self {
        Self { backend, secrets }
    }

    fn secret_key(ws: &WorkspaceId, vault: &VaultId, id: &CredentialId) -> String {
        format!("secrets/{}/{}/{}", ws.0, vault.as_str(), id.as_str())
    }

    async fn require_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<(), StoreError> {
        let exists: Option<(i32,)> =
            sqlx::query_as("select 1 from vaults where workspace_id = $1 and vault_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
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
            Some(a) => self
                .secrets
                .put(&key, Bytes::from(a.seal().into_bytes()))
                .await
                .map_err(blob_be),
            // Archived → secret purged. Delete is idempotent (NotFound ok).
            None => match self.secrets.delete(&key).await {
                Ok(()) | Err(agentd_blobs::BlobError::NotFound(_)) => Ok(()),
                Err(e) => Err(blob_be(e)),
            },
        }
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

fn blob_be(e: agentd_blobs::BlobError) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[async_trait]
impl VaultsStore for PgVaultsStore {
    async fn insert_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        sqlx::query(
            "insert into vaults (workspace_id, vault_id, display_name, created_at, doc)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(&ws.0)
        .bind(vault.id.as_str())
        .bind(&vault.display_name)
        .bind(vault.created_at.as_datetime())
        .bind(Json(&vault))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        let row: Option<(Json<Vault>,)> =
            sqlx::query_as("select doc from vaults where workspace_id = $1 and vault_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::VaultNotFound)
    }

    async fn replace_vault(&self, ws: &WorkspaceId, vault: Vault) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update vaults set display_name = $3, doc = $4
             where workspace_id = $1 and vault_id = $2",
        )
        .bind(&ws.0)
        .bind(vault.id.as_str())
        .bind(&vault.display_name)
        .bind(Json(&vault))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::VaultNotFound);
        }
        Ok(())
    }

    async fn delete_vault(&self, ws: &WorkspaceId, id: &VaultId) -> Result<Vault, StoreError> {
        let creds = self.list_credentials(ws, id).await.unwrap_or_default();
        let row: Option<(Json<Vault>,)> = sqlx::query_as(
            "delete from vaults where workspace_id = $1 and vault_id = $2 returning doc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let vault = row.map(|(d,)| d.0).ok_or(StoreError::VaultNotFound)?;
        sqlx::query("delete from credentials where workspace_id = $1 and vault_id = $2")
            .bind(&ws.0)
            .bind(id.as_str())
            .execute(&self.backend.pool)
            .await
            .map_err(be)?;
        for c in creds {
            self.put_secret(ws, id, &c.wire.id, &None).await?;
        }
        Ok(vault)
    }

    async fn list_vaults(&self, ws: &WorkspaceId) -> Result<Vec<Vault>, StoreError> {
        let rows: Vec<(Json<Vault>,)> = sqlx::query_as(
            "select doc from vaults where workspace_id = $1 order by created_at",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        self.require_vault(ws, vault).await?;
        self.put_secret(ws, vault, &cred.wire.id, &cred.auth).await?;
        sqlx::query(
            "insert into credentials
               (workspace_id, vault_id, credential_id, display_name, has_secret, created_at, doc)
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&ws.0)
        .bind(vault.as_str())
        .bind(cred.wire.id.as_str())
        .bind(&cred.wire.display_name)
        .bind(cred.auth.is_some())
        .bind(cred.wire.created_at.as_datetime())
        .bind(Json(&cred.wire))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<StoredCredential, StoreError> {
        let row: Option<(Json<Credential>, bool)> = sqlx::query_as(
            "select doc, has_secret from credentials
             where workspace_id = $1 and vault_id = $2 and credential_id = $3",
        )
        .bind(&ws.0)
        .bind(vault.as_str())
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        let (wire, has_secret) = row.ok_or(StoreError::CredentialNotFound)?;
        let auth = if has_secret {
            let sealed = self
                .secrets
                .get(&Self::secret_key(ws, vault, id))
                .await
                .map_err(blob_be)?;
            CredentialAuthInput::unseal(&String::from_utf8_lossy(&sealed))
        } else {
            None
        };
        Ok(StoredCredential { wire: wire.0, auth })
    }

    async fn replace_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
    ) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update credentials set display_name = $4, has_secret = $5, doc = $6
             where workspace_id = $1 and vault_id = $2 and credential_id = $3",
        )
        .bind(&ws.0)
        .bind(vault.as_str())
        .bind(cred.wire.id.as_str())
        .bind(&cred.wire.display_name)
        .bind(cred.auth.is_some())
        .bind(Json(&cred.wire))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
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
        sqlx::query(
            "delete from credentials
             where workspace_id = $1 and vault_id = $2 and credential_id = $3",
        )
        .bind(&ws.0)
        .bind(vault.as_str())
        .bind(id.as_str())
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        self.put_secret(ws, vault, id, &None).await?;
        Ok(existing)
    }

    async fn list_credentials(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
    ) -> Result<Vec<StoredCredential>, StoreError> {
        self.require_vault(ws, vault).await?;
        // Secret-free listing: `auth` is fetched per-credential on demand
        // (egress injection reads one at a time) — never bulk-loaded here.
        let rows: Vec<(Json<Credential>,)> = sqlx::query_as(
            "select doc from credentials where workspace_id = $1 and vault_id = $2
             order by created_at",
        )
        .bind(&ws.0)
        .bind(vault.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows
            .into_iter()
            .map(|(d,)| StoredCredential {
                wire: d.0,
                auth: None,
            })
            .collect())
    }
}
