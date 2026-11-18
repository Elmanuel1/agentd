//! A credential write claimed on the row's `updated_at`, for the two writers
//! that read a credential before they change it: the loop's refresh of an
//! `mcp_oauth` token and the API's update. The claim moves `updated_at` from
//! what the writer read, so a second claim of the same read matches nothing
//! (MC-7, MC-23), and the claimed write lands only while the claim holds.

use ::vaults::store::{ClaimedWrite, CredentialClaims, StoreError, StoredCredential};
use ::vaults::types::{CredentialId, VaultId};
use async_trait::async_trait;
use diesel::sql_types::{Jsonb, Text};
use diesel_async::RunQueryDsl;
use shared::{Timestamp, WorkspaceId};

use crate::entities::vaults::PgVaultsStore;
use crate::{diesel_be, pool_be, to_doc};

const KEY: &str = "workspace_id = $1 AND vault_id = $2 AND credential_id = $3";

#[async_trait]
impl CredentialClaims for PgVaultsStore {
    async fn claim_credential(
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

    async fn write_claimed(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        cred: &StoredCredential,
        claimed: Timestamp,
    ) -> Result<ClaimedWrite, StoreError> {
        let id = &cred.wire.id;
        if let Err(e) = self.put_secret(ws, vault, id, &cred.auth).await {
            // A failed write can still have stored the value (SSM tags after
            // it), so an archive or delete since the claim keeps no secret.
            if let Err(left) = self.settle(ws, vault, id).await {
                tracing::error!(workspace = %ws, vault = %vault, credential = %id, error = %left,
                    "vaults: a failed claimed write could not check its credential; delete its secret by hand if it is gone");
            }
            return Err(e);
        }
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let affected = diesel::sql_query(format!(
            "UPDATE credentials SET doc = $5, display_name = $6 WHERE {KEY} AND doc->>'updated_at' = $4 \
             AND doc->>'archived_at' IS NULL"
        ))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(vault.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(claimed.to_wire())
        .bind::<Jsonb, _>(to_doc(&cred.wire)?)
        .bind::<Text, _>(&cred.wire.display_name)
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        drop(conn);
        if affected == 1 {
            return Ok(ClaimedWrite::Matched);
        }
        self.settle(ws, vault, id).await
    }
}

impl PgVaultsStore {
    /// An update came between the claim and the write, or the credential was
    /// archived or deleted meanwhile, which takes its secret out.
    async fn settle(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<ClaimedWrite, StoreError> {
        match self.credential_row(ws, vault, id).await {
            Ok((wire, _)) if wire.archived_at.is_none() => Ok(ClaimedWrite::Changed),
            Ok(_) | Err(StoreError::CredentialNotFound) => {
                self.put_secret(ws, vault, id, &None).await?;
                Ok(ClaimedWrite::Gone)
            }
            Err(e) => Err(e),
        }
    }
}
