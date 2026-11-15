//! A deployment's session reaches only the MCP hosts its environment's
//! networking allows (MC-18): the fire is refused before the session exists.

use agents::types::McpServer;
use agents::AgentSnapshot;
use environments::types::EnvironmentConfig;
use shared::net::host_of;
use shared::ApiError;

/// Live's description of a fire refused for a blocked MCP host (B14).
pub const MCP_EGRESS_BLOCKED: &str =
    "An MCP server host used by the deployment's agent is blocked by the environment's network policy";

/// Refuse `agent` in `environment` when one of its MCP server hosts is
/// blocked; a config that does not read refuses nothing here, as the
/// session's own checks read it.
pub(super) fn check_mcp_egress(
    agent: &AgentSnapshot,
    environment: &serde_json::Value,
) -> Result<(), ApiError> {
    let Ok(config) = serde_json::from_value::<EnvironmentConfig>(environment.clone()) else { return Ok(()) };
    let blocked = agent
        .config
        .mcp_servers
        .iter()
        .any(|McpServer::Url { url, .. }| !host_of(url).is_some_and(|h| config.mcp_allowed(&h)));
    match blocked {
        true => Err(ApiError::invalid_request(MCP_EGRESS_BLOCKED)),
        false => Ok(()),
    }
}
