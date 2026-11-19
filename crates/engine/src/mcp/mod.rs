//! An agent's MCP servers, reached from agentd, never from a worker (MC-1).
//! Each step reads the session's stored connections and credentials once,
//! connects only the servers that need it, and calls MCP tools itself: an
//! MCP call queues no work item.

use shared::ports::BoxFuture;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use environments::types::EnvironmentConfig;
use sessions::mcp::McpConnections;
use shared::net::Guard;
use shared::{BackendError, OrgId, WorkspaceId};
use vaults::VaultsService;

pub mod client;
mod errors;
mod probe;
pub mod server;
mod servers;

pub use errors::{CONNECTION_FAILED, is_mcp_error};
pub use probe::Probe;
pub use servers::{CallOutcome, McpTool, Servers};

/// The session's environment, as the MCP networking check reads it.
pub trait McpEnvironments: Send + Sync {
    /// The config of `environment_id`, the environment of `session` in `ws`.
    fn config<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        environment_id: &'a str,
    ) -> BoxFuture<'a, Result<EnvironmentConfig, BackendError>>;
    /// The organization of `session` in `ws`.
    fn org<'a>(&'a self, ws: &'a WorkspaceId, session: &'a str)
    -> BoxFuture<'a, Result<OrgId, BackendError>>;
}

/// What an adapter implements to provide [`McpEnvironments`]; the methods are documented there.
pub trait NativeMcpEnvironments: Send + Sync {
    fn config(
        &self,
        ws: &WorkspaceId,
        session: &str,
        environment_id: &str,
    ) -> impl Future<Output = Result<EnvironmentConfig, BackendError>> + Send;
    fn org(
        &self,
        ws: &WorkspaceId,
        session: &str,
    ) -> impl Future<Output = Result<OrgId, BackendError>> + Send;
}

impl<T: NativeMcpEnvironments> McpEnvironments for T {
    fn config<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
        environment_id: &'a str,
    ) -> BoxFuture<'a, Result<EnvironmentConfig, BackendError>> {
        Box::pin(NativeMcpEnvironments::config(self, ws, session, environment_id))
    }
    fn org<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a str,
    ) -> BoxFuture<'a, Result<OrgId, BackendError>> {
        Box::pin(NativeMcpEnvironments::org(self, ws, session))
    }
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
