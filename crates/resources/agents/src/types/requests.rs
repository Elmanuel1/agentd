use serde::Deserialize;
use shared::{Metadata, MetadataPatch, Patch};

use super::{ExecutionIdentity, McpServer, ModelInput, MultiagentParams, SkillParams, ToolParams};

/// `POST /v1/agents` body (validation happens in [`super::AgentConfig`] so
/// error strings match the live API exactly; unknown fields are rejected like
/// live).
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
    pub tools: Vec<ToolParams>,
    /// Optional.
    #[serde(default)]
    pub mcp_servers: Vec<McpServer>,
    /// Optional.
    #[serde(default)]
    pub skills: Vec<SkillParams>,
    /// Optional.
    pub multiagent: Option<MultiagentParams>,
    /// Optional; `null` is the default service account.
    #[serde(default)]
    pub execution_identity: Option<ExecutionIdentity>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}

/// `POST /v1/agents/{id}` body. A field left out keeps its value; `null`
/// clears it (lists to `[]`, `execution_identity` to the service account).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentRequest {
    /// Optimistic lock: must equal the current version when sent.
    pub version: Option<u64>,
    /// Replace name.
    pub name: Option<String>,
    /// Replace model.
    pub model: Option<ModelInput>,
    /// Replace or clear the description.
    #[serde(default)]
    pub description: Patch<String>,
    /// Replace or clear the system prompt.
    #[serde(default)]
    pub system: Patch<String>,
    /// Replace or clear tools.
    #[serde(default)]
    pub tools: Patch<Vec<ToolParams>>,
    /// Replace or clear MCP servers.
    #[serde(default)]
    pub mcp_servers: Patch<Vec<McpServer>>,
    /// Replace or clear skills.
    #[serde(default)]
    pub skills: Patch<Vec<SkillParams>>,
    /// Replace, merge (`multiagent_20261001` onto the same type) or clear.
    #[serde(default)]
    pub multiagent: Patch<MultiagentParams>,
    /// Replace or restore the default.
    #[serde(default)]
    pub execution_identity: Patch<ExecutionIdentity>,
    /// Merge: strings upsert, `null` removes.
    #[serde(default)]
    pub metadata: Patch<MetadataPatch>,
}
