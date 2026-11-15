//! The loop's refresh of an `mcp_oauth` token, guarded by the credential
//! row's `updated_at`: the claim moves it, so a second worker's claim of the
//! same read matches nothing (MC-7, MC-23), and the final write lands only
//! while the claim's time is still there.

use ::vaults::store::{RefreshWrite, StoreError, StoredCredential};
use ::vaults::types::{CredentialId, VaultId};
use diesel::sql_types::{Jsonb, Text};
use diesel_async::RunQueryDsl;
use shared::{Timestamp, WorkspaceId};

use crate::entities::vaults::PgVaultsStore;
use crate::{diesel_be, pool_be, to_doc};

const KEY: &str = "workspace_id = $1 AND vault_id = $2 AND credential_id = $3";

impl PgVaultsStore {
    pub(super) async fn claim_refresh_row(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
        read: Timestamp,
        now: Timestamp,
    ) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::sql_query(format!(
            "UPDATE credentials SET doc = jsonb_set(doc, '{{updated_at}}', to_jsonb($5::text)) \
             WHERE {KEY} AND doc->>'updated_at' = $4 AND doc->>'archived_at' IS NULL"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(vault.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(read.to_wire())
        .bind::<Text, _>(now.to_wire())
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(affected == 1)
    }

    pub(super) async fn finish_refresh_row(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: &StoredCredential,
        claimed: Timestamp,
    ) -> Result<RefreshWrite, StoreError> {
        let id = &cred.wire.id;
        self.put_secret(ws, vault, id, &cred.auth).await?;
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::sql_query(format!(
            "UPDATE credentials SET doc = $5 WHERE {KEY} AND doc->>'updated_at' = $4 AND doc->>'archived_at' IS NULL"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(vault.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(claimed.to_wire())
        .bind::<Jsonb, _>(to_doc(&cred.wire)?)
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        drop(conn);
        if affected == 1 {
            return Ok(RefreshWrite::Matched);
        }
        // An update came between the claim and this write, or the credential
        // was archived or deleted meanwhile, which takes its secret out.
        match self.credential_row(ws, vault, id).await {
            Ok((wire, _)) if wire.archived_at.is_none() => Ok(RefreshWrite::Changed),
            Ok(_) | Err(StoreError::CredentialNotFound) => {
                self.put_secret(ws, vault, id, &None).await?;
                Ok(RefreshWrite::Gone)
            }
            Err(e) => Err(e),
        }
    }
}
