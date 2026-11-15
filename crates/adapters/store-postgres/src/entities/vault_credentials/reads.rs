//! The reads of a session's credentials, each one statement: the loop's MCP
//! credentials, keyed by the agent's server URLs, and the work secret's
//! `environment_variable` ones.

use ::vaults::store::{CredentialReads, StoreError};
use ::vaults::types::{Credential, CredentialAuthInput, CredentialId, VaultId};
use async_trait::async_trait;
use diesel::dsl::sql;
use diesel::sql_types::{Array, Bool, Text};
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use crate::entities::vaults::PgVaultsStore;
use crate::schema::credentials;
use crate::{diesel_be, parse_doc, pool_be};

#[async_trait]
impl CredentialReads for PgVaultsStore {
    async fn mcp_credentials(
        &self,
        ws: &WorkspaceId,
        vaults: &[String],
        urls: &[String],
    ) -> Result<Vec<Credential>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = credentials::table
            .filter(credentials::workspace_id.eq(ws.as_str()))
            .filter(credentials::vault_id.eq_any(vaults))
            .filter(
                sql::<Bool>("doc->>'archived_at' IS NULL AND doc->'auth'->>'mcp_server_url' = ANY(")
                    .bind::<Array<Text>, _>(urls)
                    .sql(")"),
            )
            .select(credentials::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
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

    async fn credential_secret(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<CredentialAuthInput, StoreError> {
        self.read_secret(ws, vault, id).await
    }
}
