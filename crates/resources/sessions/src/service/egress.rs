//! A deployment's session reaches only the MCP hosts its environment's
//! networking allows (MC-18): the fire is refused before the session exists.

use agents::AgentSnapshot;
use agents::types::McpServer;
use environments::types::EnvironmentConfig;
use shared::ApiError;
use shared::net::host_of;

/// Live's description of a fire refused for a blocked MCP host (B14).
pub const MCP_EGRESS_BLOCKED: &str =
    "An MCP server host used by the deployment's agent is blocked by the environment's network policy";

/// Why a deployment's session was not created.
#[derive(Debug, Clone)]
pub enum FireRefusal {
    /// An MCP server host of the agent is blocked by the environment (MC-19).
    EgressBlocked,
    /// Any other refusal, as a session create answers it.
    Api(ApiError),
}

impl From<ApiError> for FireRefusal {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl FireRefusal {
    /// As the API would answer it.
    pub fn into_api(self) -> ApiError {
        match self {
            Self::EgressBlocked => ApiError::invalid_request(MCP_EGRESS_BLOCKED),
            Self::Api(e) => e,
        }
    }
}

/// Refuse `agent` in `environment` when one of its MCP server hosts is
/// blocked; a config that does not read refuses nothing here, as the
/// session's own checks read it.
pub(super) fn check_mcp_egress(
    agent: &AgentSnapshot,
    environment: &serde_json::Value,
) -> Result<(), FireRefusal> {
    let Ok(config) = serde_json::from_value::<EnvironmentConfig>(environment.clone()) else { return Ok(()) };
    let blocked = agent
        .config
        .mcp_servers
        .iter()
        .any(|McpServer::Url { url, .. }| !host_of(url).is_some_and(|h| config.mcp_allowed(&h)));
    match blocked {
        true => Err(FireRefusal::EgressBlocked),
        false => Ok(()),
    }
}
