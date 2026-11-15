//! The reads of a session's credentials, each one statement: the loop's MCP
//! credentials, keyed by the agent's server URLs, and the work secret's
//! `environment_variable` ones.

use ::vaults::store::StoreError;
use ::vaults::types::Credential;
use diesel::dsl::sql;
use diesel::sql_types::{Array, Bool, Text};
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use crate::entities::vaults::PgVaultsStore;
use crate::schema::credentials;
use crate::{diesel_be, parse_doc, pool_be};

impl PgVaultsStore {
    pub(super) async fn mcp_rows(
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

    pub(super) async fn environment_rows(
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
