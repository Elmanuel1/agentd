use agents::types::{McpServer, ModelInput, SkillParams, ToolParams};
use agents::AgentSnapshot;
use serde::Deserialize;
use shared::limits::{validate_array_max, values};
use shared::{ApiError, Metadata, MetadataPatch, Patch};

use super::{Budget, SessionResourceInput};
use crate::events::UserEvent;

/// The `agent` of a create: an id, a pinned reference, or a reference with
/// overrides that apply to this session only.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AgentRefInput {
    /// A bare agent id: its latest version.
    Shorthand(String),
    /// `{type:"agent"|"agent_with_overrides", id, version?, …overrides}`.
    Tagged(AgentOverrides),
}

/// A tagged agent reference; with `agent_with_overrides`, the fields it sends
/// replace the agent's own for this session.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOverrides {
    /// `agent` or `agent_with_overrides`.
    #[serde(rename = "type")]
    pub tag: AgentRefTag,
    /// Agent id.
    pub id: String,
    /// Pinned version; latest when left out.
    #[serde(default)]
    pub version: Option<u64>,
    /// Override the model.
    #[serde(default)]
    pub model: Option<ModelInput>,
    /// Override (or clear) the system prompt.
    #[serde(default)]
    pub system: Patch<String>,
    /// Override the tools.
    #[serde(default)]
    pub tools: Option<Vec<ToolParams>>,
    /// Override the MCP servers.
    #[serde(default)]
    pub mcp_servers: Option<Vec<McpServer>>,
    /// Override the skills.
    #[serde(default)]
    pub skills: Option<Vec<SkillParams>>,
}

/// The `type` of a tagged agent reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRefTag {
    /// A plain reference: overrides are not allowed.
    Agent,
    /// A reference with overrides.
    AgentWithOverrides,
}

impl AgentRefInput {
    /// The agent id.
    pub fn id(&self) -> &str {
        match self {
            Self::Shorthand(id) => id,
            Self::Tagged(o) => &o.id,
        }
    }

    /// The pinned version, if any.
    pub fn version(&self) -> Option<u64> {
        match self {
            Self::Shorthand(_) => None,
            Self::Tagged(o) => o.version,
        }
    }
}

/// `POST /v1/sessions` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSessionRequest {
    /// Required.
    pub agent: Option<AgentRefInput>,
    /// Required.
    pub environment_id: Option<String>,
    /// Optional.
    pub title: Option<String>,
    /// Optional.
    #[serde(default)]
    pub resources: Vec<SessionResourceInput>,
    /// Optional.
    #[serde(default)]
    pub vault_ids: Vec<String>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
    /// Optional spending limit.
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Events queued as the session's first input (`user.message`,
    /// `user.define_outcome`).
    #[serde(default)]
    pub initial_events: Vec<UserEvent>,
    /// Set by a deployment fire; never read from the wire.
    #[serde(skip)]
    pub deployment_id: Option<String>,
}

/// The agent fields a session update may change (only while not running).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentUpdate {
    /// Replace the tools.
    #[serde(default)]
    pub tools: Option<Vec<ToolParams>>,
    /// Replace the MCP servers.
    #[serde(default)]
    pub mcp_servers: Option<Vec<McpServer>>,
}

/// `POST /v1/sessions/{id}` body: a field left out keeps its value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSessionRequest {
    /// Replace or clear the title.
    #[serde(default)]
    pub title: Patch<String>,
    /// Merge: strings upsert, `null` removes; `null` for the whole map clears it.
    #[serde(default)]
    pub metadata: Patch<MetadataPatch>,
    /// Change the session's agent.
    #[serde(default)]
    pub agent: Option<AgentUpdate>,
    /// Replace or remove the spending limit.
    #[serde(default)]
    pub budget: Patch<Budget>,
    /// Not yet supported (live rejects it).
    #[serde(default)]
    pub vault_ids: Option<Vec<String>>,
}

fn unknown_field(name: &str) -> ApiError {
    ApiError::invalid_request(format!("Failed to parse request body: unknown field \"{name}\""))
}

fn resolve_tools(tools: Vec<ToolParams>) -> Result<Vec<agents::types::Tool>, ApiError> {
    validate_array_max("tools", tools.len(), values::TOOLS_MAX)?;
    tools.into_iter().map(ToolParams::resolve).collect()
}

impl AgentOverrides {
    /// `snapshot` with this reference's overrides applied. A plain `agent`
    /// reference carries none.
    pub fn apply(self, mut snapshot: AgentSnapshot) -> Result<AgentSnapshot, ApiError> {
        if self.tag == AgentRefTag::Agent {
            let sent = [
                ("model", self.model.is_some()),
                ("system", self.system != Patch::Keep),
                ("tools", self.tools.is_some()),
                ("mcp_servers", self.mcp_servers.is_some()),
                ("skills", self.skills.is_some()),
            ];
            return match sent.iter().find(|(_, set)| *set) {
                Some((name, _)) => Err(unknown_field(name)),
                None => Ok(snapshot),
            };
        }
        let c = &mut snapshot.config;
        if let Some(model) = self.model {
            c.model = model.canonicalize();
        }
        c.system = self.system.apply_to_option(c.system.take());
        if let Some(tools) = self.tools {
            c.tools = resolve_tools(tools)?;
        }
        if let Some(servers) = self.mcp_servers {
            c.mcp_servers = servers;
        }
        if let Some(skills) = self.skills {
            c.skills = skills.into_iter().map(SkillParams::resolve).collect();
        }
        Ok(snapshot)
    }
}

impl AgentUpdate {
    /// `snapshot` with this update's tools and MCP servers.
    pub fn apply(self, mut snapshot: AgentSnapshot) -> Result<AgentSnapshot, ApiError> {
        if let Some(tools) = self.tools {
            snapshot.config.tools = resolve_tools(tools)?;
        }
        if let Some(servers) = self.mcp_servers {
            snapshot.config.mcp_servers = servers;
        }
        Ok(snapshot)
    }
}
