//! What the loop keeps of a session's MCP servers between steps: each
//! server's `Mcp-Session-Id` and tool list, per agentd session (MC-13).

use shared::ports::BoxFuture;
use shared::{Timestamp, WorkspaceId};
use std::future::Future;

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
pub trait McpConnections: Send + Sync {
    /// The session's connections, never another session's (MC-17).
    fn connections<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<McpConnection>, StoreError>>;
    /// Store `connection` for the session, replacing the server's row.
    fn save<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        connection: &'a McpConnection,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
}

/// What an adapter implements to provide [`McpConnections`]; the methods are documented there.
pub trait McpConnectionsImpl: Send + Sync {
    fn connections(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> impl Future<Output = Result<Vec<McpConnection>, StoreError>> + Send;
    fn save(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        connection: &McpConnection,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}

impl<T: McpConnectionsImpl> McpConnections for T {
    fn connections<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<McpConnection>, StoreError>> {
        Box::pin(McpConnectionsImpl::connections(self, ws, session))
    }
    fn save<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        connection: &'a McpConnection,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(McpConnectionsImpl::save(self, ws, session, connection))
    }
}
