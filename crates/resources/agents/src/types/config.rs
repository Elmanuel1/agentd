use serde::{Deserialize, Serialize};
use shared::{Metadata, Timestamp};

use super::{AgentId, AgentTag, McpServer, ModelSpec, Multiagent, SkillRef, Tool};

/// The canonical agent configuration shared by all three projections.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Human name (1–256 chars).
    pub name: String,
    /// Canonical model object.
    pub model: ModelSpec,
    /// Optional description (≤2048).
    pub description: Option<String>,
    /// Optional system prompt (≤100000).
    pub system: Option<String>,
    /// Tools (≤128).
    pub tools: Vec<Tool>,
    /// MCP servers (≤20, unique names).
    pub mcp_servers: Vec<McpServer>,
    /// Skills (≤20, unique skill_id).
    pub skills: Vec<SkillRef>,
    /// Multiagent roster.
    pub multiagent: Option<Multiagent>,
}

/// Full wire agent object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// Archive timestamp (null while active).
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// Description (nullable).
    pub description: Option<String>,
    /// `agent_…`.
    pub id: AgentId,
    /// Declared MCP servers.
    pub mcp_servers: Vec<McpServer>,
    /// User metadata.
    pub metadata: Metadata,
    /// Canonical model.
    pub model: ModelSpec,
    /// Multiagent roster.
    pub multiagent: Option<Multiagent>,
    /// Name.
    pub name: String,
    /// Skills.
    pub skills: Vec<SkillRef>,
    /// System prompt (nullable).
    pub system: Option<String>,
    /// Tools.
    pub tools: Vec<Tool>,
    /// Always `"agent"`.
    #[serde(rename = "type")]
    pub tag: AgentTag,
    /// Updated.
    pub updated_at: Timestamp,
    /// Integer version, starts at 1.
    pub version: u64,
}

impl Agent {
    /// The embedded projection used by sessions/threads (live shape: full
    /// config + id + version, **no** timestamps/metadata/archived_at).
    pub fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            description: self.description.clone(),
            id: self.id.clone(),
            mcp_servers: self.mcp_servers.clone(),
            model: self.model.clone(),
            multiagent: self.multiagent.clone(),
            name: self.name.clone(),
            skills: self.skills.clone(),
            system: self.system.clone(),
            tools: self.tools.clone(),
            tag: AgentTag::Agent,
            version: self.version,
        }
    }

    /// The compact projection used by deployments.
    pub fn reference(&self) -> AgentRef {
        AgentRef { id: self.id.clone(), tag: AgentTag::Agent, version: self.version }
    }
}

/// Frozen agent config embedded in sessions and threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Description (nullable).
    pub description: Option<String>,
    /// Agent id.
    pub id: AgentId,
    /// MCP servers.
    pub mcp_servers: Vec<McpServer>,
    /// Model.
    pub model: ModelSpec,
    /// Multiagent roster.
    pub multiagent: Option<Multiagent>,
    /// Name.
    pub name: String,
    /// Skills.
    pub skills: Vec<SkillRef>,
    /// System prompt (nullable).
    pub system: Option<String>,
    /// Tools.
    pub tools: Vec<Tool>,
    /// Always `"agent"`.
    #[serde(rename = "type")]
    pub tag: AgentTag,
    /// Pinned version.
    pub version: u64,
}

/// Compact reference: `{"id":"agent_…","type":"agent","version":1}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRef {
    /// Agent id.
    pub id: AgentId,
    /// Always `"agent"`.
    #[serde(rename = "type")]
    pub tag: AgentTag,
    /// Version.
    pub version: u64,
}
