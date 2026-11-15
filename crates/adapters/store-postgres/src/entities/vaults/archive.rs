//! A vault's archive and delete. Each locks the vault's row for update
//! (VA-8), so no credential is created or rotated in it meanwhile, changes the
//! vault and every credential in it in one transaction, and purges the
//! credentials' secrets right after the commit (VA-3). A create or rotation
//! that wrote its secret first finds the credential archived or gone and takes
//! the secret back out, so no archived credential keeps one.

use ::vaults::store::{StoreError, VaultCascade};
use ::vaults::types::{CredentialId, Vault, VaultId};
use async_trait::async_trait;
use diesel::sql_types::{Jsonb, Text};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl, QueryableByName};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::{Timestamp, WorkspaceId};

use super::PgVaultsStore;
use crate::entities::webhook_deliveries::insert_deliveries;
use crate::schema::{credentials, vaults};
use crate::{diesel_be, parse_doc, pool_be, to_doc};
use ::vaults::store::CascadeDeliveries;
use webhooks::Delivery;

#[derive(QueryableByName)]
struct Touched {
    #[diesel(sql_type = Text)]
    credential_id: String,
}

/// The vault's doc, its row locked for update.
async fn locked(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &VaultId,
) -> Result<Option<serde_json::Value>, diesel::result::Error> {
    vaults::table
        .filter(vaults::workspace_id.eq(ws.as_str()))
        .filter(vaults::vault_id.eq(id.as_str()))
        .select(vaults::doc)
        .for_update()
        .first(c)
        .await
        .optional()
}

#[async_trait]
impl VaultCascade for PgVaultsStore {
    /// Archive the vault and its unarchived credentials at `at`; an archived
    /// vault is answered as it is.
    async fn archive_vault(
        &self,
        ws: &WorkspaceId,
        id: &VaultId,
        at: Timestamp,
        deliveries: &CascadeDeliveries<'_>,
    ) -> Result<Vault, StoreError> {
        let stamp =
            serde_json::to_value(at).map_err(|e| StoreError::Backend(shared::BackendError::new(e)))?;
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let (vault, archived) = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let Some(doc) = locked(c, ws, id).await? else {
                        return Ok(Err(StoreError::VaultNotFound));
                    };
                    let mut vault: Vault = match parse_doc(doc) {
                        Ok(vault) => vault,
                        Err(e) => return Ok(Err(e)),
                    };
                    if vault.archived_at.is_some() {
                        return Ok(Ok((vault, Vec::new())));
                    }
                    (vault.archived_at, vault.updated_at) = (Some(at), at);
                    let doc = match to_doc(&vault) {
                        Ok(doc) => doc,
                        Err(e) => return Ok(Err(e)),
                    };
                    diesel::update(vaults::table.filter(vaults::workspace_id.eq(ws.as_str())))
                        .filter(vaults::vault_id.eq(id.as_str()))
                        .set(vaults::doc.eq(doc))
                        .execute(c)
                        .await?;
                    let archived: Vec<Touched> = diesel::sql_query(
                        "UPDATE credentials SET has_secret = false,
                           doc = jsonb_set(jsonb_set(doc, '{archived_at}', $3), '{updated_at}', $3)
                         WHERE workspace_id = $1 AND vault_id = $2 AND doc->>'archived_at' IS NULL
                         RETURNING credential_id",
                    )
                    .bind::<Text, _>(ws.as_str())
                    .bind::<Text, _>(id.as_str())
                    .bind::<Jsonb, _>(&stamp)
                    .load(c)
                    .await?;
                    let ids: Vec<String> = archived.iter().map(|t| t.credential_id.clone()).collect();
                    insert_deliveries(c, &deliveries(&ids)).await?;
                    Ok(Ok((vault, archived)))
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)??;
        drop(conn);
        self.purge_secrets(ws, id, archived.into_iter().map(|t| t.credential_id)).await;
        Ok(vault)
    }

    /// Delete the vault and every credential in it, archived ones included.
    async fn delete_vault(
        &self,
        ws: &WorkspaceId,
        id: &VaultId,
        deliveries: &[Delivery],
    ) -> Result<Vault, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let (doc, deleted) = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let Some(doc) = locked(c, ws, id).await? else {
                        return Ok(Err(StoreError::VaultNotFound));
                    };
                    let deleted: Vec<String> = diesel::delete(
                        credentials::table
                            .filter(credentials::workspace_id.eq(ws.as_str()))
                            .filter(credentials::vault_id.eq(id.as_str())),
                    )
                    .returning(credentials::credential_id)
                    .get_results(c)
                    .await?;
                    diesel::delete(vaults::table.filter(vaults::workspace_id.eq(ws.as_str())))
                        .filter(vaults::vault_id.eq(id.as_str()))
                        .execute(c)
                        .await?;
                    insert_deliveries(c, deliveries).await?;
                    Ok(Ok((doc, deleted)))
                }
                .scope_boxed()
            })
            .await
            .map_err(diesel_be)??;
        drop(conn);
        self.purge_secrets(ws, id, deleted).await;
        parse_doc(doc)
    }
}

impl PgVaultsStore {
    /// Delete each credential's secret. The rows are already gone or archived,
    /// so a secret that cannot be deleted is left behind for a person to remove.
    async fn purge_secrets(&self, ws: &WorkspaceId, vault: &VaultId, ids: impl IntoIterator<Item = String>) {
        for raw in ids {
            let Ok(id) = CredentialId::parse(&raw) else { continue };
            if let Err(e) = self.put_secret(ws, vault, &id, &None).await {
                tracing::error!(workspace = %ws, vault = %vault, credential = %raw, error = %e,
                    "vaults: a purged credential's secret could not be deleted; delete it by hand");
            }
        }
    }
}
