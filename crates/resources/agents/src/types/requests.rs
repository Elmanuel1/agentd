use serde::Deserialize;
use shared::Metadata;

use super::{McpServer, ModelInput, Multiagent, SkillRef, Tool};

/// `POST /v1/agents` body (validation happens in the service so error strings
/// match the live API exactly; unknown fields are rejected like live).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentRequest {
    /// Required (validated, not enforced by serde).
    pub name: Option<String>,
    /// Required.
    pub model: Option<ModelInput>,
    /// Optional.
    pub description: Option<String>,
    /// Optional.
    pub system: Option<String>,
    /// Optional.
    #[serde(default)]
    pub tools: Vec<Tool>,
    /// Optional.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// Optional.
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    /// Optional.
    pub multiagent: Option<Multiagent>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}

/// `POST /v1/agents/{id}` body — partial update; every present field replaces.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentRequest {
    /// Optimistic lock: must equal the current version when present.
    pub version: Option<u64>,
    /// Replace name.
    pub name: Option<String>,
    /// Replace model.
    pub model: Option<ModelInput>,
    /// Replace description.
    pub description: Option<String>,
    /// Replace system prompt.
    pub system: Option<String>,
    /// Replace tools.
    pub tools: Option<Vec<Tool>>,
    /// Replace MCP servers.
    pub mcp_servers: Option<Vec<McpServer>>,
    /// Replace skills.
    pub skills: Option<Vec<SkillRef>>,
    /// Replace multiagent roster.
    pub multiagent: Option<Multiagent>,
    /// Replace metadata.
    pub metadata: Option<Metadata>,
}
