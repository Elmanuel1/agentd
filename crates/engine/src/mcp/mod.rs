//! An agent's MCP servers, reached from agentd, never from a worker (MC-1).
//! Each step reads the session's stored connections and credentials once,
//! connects only the servers that need it, and calls MCP tools itself: an
//! MCP call queues no work item.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use environments::types::EnvironmentConfig;
use sessions::mcp::McpConnections;
use shared::net::Guard;
use shared::{BackendError, WorkspaceId};
use vaults::VaultsService;

pub mod client;
mod errors;
pub mod server;
mod servers;

pub use errors::{is_mcp_error, CONNECTION_FAILED};
pub use servers::{CallOutcome, McpTool, Servers};

/// The session's environment, as the MCP networking check reads it.
#[async_trait]
pub trait McpEnvironments: Send + Sync {
    /// The config of `environment_id`, the environment of `session` in `ws`.
    async fn config(
        &self,
        ws: &WorkspaceId,
        session: &str,
        environment_id: &str,
    ) -> Result<EnvironmentConfig, BackendError>;
}

/// What the loop reaches MCP servers with.
#[derive(Clone)]
pub struct Mcp {
    /// The stored connections.
    pub connections: Arc<dyn McpConnections>,
    /// The session's vault credentials.
    pub vaults: Arc<VaultsService>,
    /// The session's environment.
    pub environments: Arc<dyn McpEnvironments>,
    /// Where agentd may connect.
    pub guard: Guard,
    /// Each request's deadline.
    pub timeout: Duration,
}

impl std::fmt::Debug for Mcp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mcp")
            .field("guard", &self.guard)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}
