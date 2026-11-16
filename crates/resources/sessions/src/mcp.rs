//! What the loop keeps of a session's MCP servers between steps: each
//! server's `Mcp-Session-Id` and tool list, per agentd session (MC-13).

use async_trait::async_trait;
use shared::{Timestamp, WorkspaceId};

use crate::store::StoreError;
use crate::types::SessionId;

/// One server's connection, as the last reconnect left it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpConnection {
    /// The server's `url`, as the agent declares it.
    pub server_url: String,
    /// The session id the server gave at `initialize`, when it gave one.
    pub mcp_session_id: Option<String>,
    /// Its `tools/list` answer: the tools, as the server describes them.
    pub tools: serde_json::Value,
    /// The `updated_at` of the credential the connection was made with.
    pub credential_updated_at: Option<Timestamp>,
}

/// The `mcp_connections` rows. A session's rows go with its delete, in the
/// delete's commit (MC-22).
#[async_trait]
pub trait McpConnections: Send + Sync {
    /// The session's connections, never another session's (MC-17).
    async fn connections(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> Result<Vec<McpConnection>, StoreError>;
    /// Store `connection` for the session, replacing the server's row.
    async fn save(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        connection: &McpConnection,
    ) -> Result<(), StoreError>;
}
