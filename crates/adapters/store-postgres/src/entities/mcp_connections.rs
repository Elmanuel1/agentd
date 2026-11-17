//! [`McpConnections`] on Postgres: the `mcp_connections` rows (V9), keyed by
//! workspace, session and server URL.

use ::sessions::mcp::{McpConnection, McpConnectionsImpl};
use ::sessions::store::StoreError;
use ::sessions::types::SessionId;
use chrono::{DateTime, Utc};
use diesel::sql_types::{Jsonb, Nullable, Text, Timestamptz};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::{Timestamp, WorkspaceId};

use crate::{diesel_be, pool_be, PgBackend};

/// Postgres-backed MCP connections.
#[derive(Debug, Clone)]
pub struct PgMcpConnections {
    backend: PgBackend,
}

impl PgMcpConnections {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = Text)]
    server_url: String,
    #[diesel(sql_type = Nullable<Text>)]
    mcp_session_id: Option<String>,
    #[diesel(sql_type = Jsonb)]
    tools: serde_json::Value,
    #[diesel(sql_type = Nullable<Timestamptz>)]
    credential_updated_at: Option<DateTime<Utc>>,
}

/// The session's connections go with the session, in its delete's transaction.
pub(crate) async fn forget(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
) -> Result<(), diesel::result::Error> {
    diesel::sql_query("DELETE FROM mcp_connections WHERE workspace_id = $1 AND session_id = $2")
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .execute(c)
        .await
        .map(drop)
}

impl McpConnectionsImpl for PgMcpConnections {
    async fn connections(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> Result<Vec<McpConnection>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let rows: Vec<Row> = diesel::sql_query(
            "SELECT server_url, mcp_session_id, tools, credential_updated_at FROM mcp_connections \
             WHERE workspace_id = $1 AND session_id = $2",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(session.as_str())
        .load(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(rows
            .into_iter()
            .map(|r| McpConnection {
                server_url: r.server_url,
                mcp_session_id: r.mcp_session_id,
                tools: r.tools,
                credential_updated_at: r.credential_updated_at.map(Timestamp::from_datetime),
            })
            .collect())
    }

    async fn save(&self, ws: &WorkspaceId, session: &SessionId, c: &McpConnection) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        diesel::sql_query(
            "INSERT INTO mcp_connections \
             (workspace_id, session_id, server_url, mcp_session_id, tools, credential_updated_at, connected_at) \
             VALUES ($1, $2, $3, $4, $5, $6, now()) \
             ON CONFLICT (workspace_id, session_id, server_url) DO UPDATE SET \
             mcp_session_id = excluded.mcp_session_id, tools = excluded.tools, \
             credential_updated_at = excluded.credential_updated_at, connected_at = excluded.connected_at",
        )
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(session.as_str())
        .bind::<Text, _>(&c.server_url)
        .bind::<Nullable<Text>, _>(c.mcp_session_id.as_deref())
        .bind::<Jsonb, _>(&c.tools)
        .bind::<Nullable<Timestamptz>, _>(c.credential_updated_at.map(|t| t.as_datetime()))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(())
    }
}
