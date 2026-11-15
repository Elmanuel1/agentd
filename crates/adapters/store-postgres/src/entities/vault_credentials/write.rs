//! A credential's create and rotation. Each writes its secret first, then its
//! row: a create under the vault's row locked for share (VA-7), so it waits
//! for an archive or delete in progress; a rotation only while the credential
//! is unarchived. A row the write cannot make takes its secret back out, so a
//! secret written while the vault was archived or deleted never stays.

use ::vaults::store::{CredentialWrites, StoreError, StoredCredential};
use ::vaults::types::{CredentialId, Vault, VaultId};
use async_trait::async_trait;
use diesel::dsl::sql;
use diesel::sql_types::Bool;
use diesel::{BoolExpressionMethods, ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::WorkspaceId;
use webhooks::Delivery;

use super::NewCredential;
use crate::entities::vaults::PgVaultsStore;
use crate::entities::webhook_deliveries::insert_deliveries;
use crate::schema::{credentials, vaults};
use crate::{diesel_be, parse_doc, pool_be, to_doc};

#[async_trait]
impl CredentialWrites for PgVaultsStore {
    async fn insert_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
        facts: &[Delivery],
    ) -> Result<(), StoreError> {
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
        let inserted: Result<(), StoreError> = async {
            let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
            conn.transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let doc: Option<serde_json::Value> = vaults::table
                        .filter(vaults::workspace_id.eq(ws.as_str()))
                        .filter(vaults::vault_id.eq(vault.as_str()))
                        .select(vaults::doc)
                        .for_share()
                        .first(c)
                        .await
                        .optional()?;
                    let open =
                        doc.map(|d| parse_doc::<Vault, StoreError>(d).map(|v| v.archived_at.is_none()));
                    match open {
                        None => return Ok(Err(StoreError::VaultNotFound)),
                        Some(Err(e)) => return Ok(Err(e)),
                        Some(Ok(false)) => return Ok(Err(StoreError::VaultArchived)),
                        Some(Ok(true)) => {}
                    }
                    diesel::insert_into(credentials::table).values(row).execute(c).await?;
                    insert_deliveries(c, facts).await?;
                    Ok(Ok(()))
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?
        }
        .await;
        if inserted.is_err() {
            self.put_secret(ws, vault, &cred.wire.id, &None).await?;
        }
        inserted
    }

    async fn archive_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: StoredCredential,
        facts: &[Delivery],
    ) -> Result<(), StoreError> {
        let rotates = cred.auth.is_some();
        if rotates {
            self.put_secret(ws, vault, &cred.wire.id, &cred.auth).await?;
        }
        let key = credentials::workspace_id
            .eq(ws.as_str())
            .and(credentials::vault_id.eq(vault.as_str()))
            .and(credentials::credential_id.eq(cred.wire.id.as_str()));
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let doc = to_doc(&cred.wire)?;
        let name = &cred.wire.display_name;
        let affected = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let affected = diesel::update(credentials::table.filter(key))
                        .filter(sql::<Bool>("doc->>'archived_at' IS NULL"))
                        .set((
                            credentials::display_name.eq(name),
                            credentials::has_secret.eq(rotates),
                            credentials::doc.eq(doc),
                        ))
                        .execute(c)
                        .await?;
                    if affected == 1 {
                        insert_deliveries(c, facts).await?;
                    }
                    Ok(affected)
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        if affected == 1 {
            drop(conn);
            // An archive's own write: the secret goes right after the commit.
            return match rotates {
                true => Ok(()),
                false => self.put_secret(ws, vault, &cred.wire.id, &None).await,
            };
        }
        let exists: Option<String> = credentials::table
            .filter(key)
            .select(credentials::credential_id)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        drop(conn);
        if rotates {
            self.put_secret(ws, vault, &cred.wire.id, &None).await?;
        }
        Err(exists.map_or(StoreError::CredentialNotFound, |_| StoreError::CredentialArchived))
    }

    /// Delete one credential with its delivery, in one statement whose
    /// returned row is the answer, so a repeated delete is `404` and sends no
    /// second delivery; then its secret, which may already be gone.
    async fn delete_credential(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        facts: &[Delivery],
    ) -> Result<StoredCredential, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let doc: Option<serde_json::Value> = diesel::delete(
                        credentials::table
                            .filter(credentials::workspace_id.eq(ws.as_str()))
                            .filter(credentials::vault_id.eq(vault.as_str()))
                            .filter(credentials::credential_id.eq(id.as_str())),
                    )
                    .returning(credentials::doc)
                    .get_result(c)
                    .await
                    .optional()?;
                    if doc.is_some() {
                        insert_deliveries(c, facts).await?;
                    }
                    Ok(doc)
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)?;
        drop(conn);
        let wire = parse_doc(doc.ok_or(StoreError::CredentialNotFound)?)?;
        self.put_secret(ws, vault, id, &None).await?;
        Ok(StoredCredential { wire, auth: None })
    }

    async fn send(&self, sent: &[Delivery]) -> Result<(), StoreError> {
        crate::entities::webhook_deliveries::publish(&self.backend, sent).await.map_err(StoreError::Backend)
    }
}
