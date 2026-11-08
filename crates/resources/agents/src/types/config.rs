use serde::{Deserialize, Serialize};
use shared::{Metadata, Timestamp};

use super::{AgentId, AgentTag, ExecutionIdentity, McpServer, ModelSpec, Multiagent, SkillRef, Tool};

/// The canonical agent configuration, written once and flattened into each
/// projection ([`Agent`], [`AgentSnapshot`]).
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
    /// Tools (≤256).
    pub tools: Vec<Tool>,
    /// MCP servers (≤20, unique names).
    pub mcp_servers: Vec<McpServer>,
    /// Skills (≤20, unique skill_id).
    pub skills: Vec<SkillRef>,
    /// Multiagent configuration.
    pub multiagent: Option<Multiagent>,
    /// The principal its runs execute as.
    #[serde(default)]
    pub execution_identity: ExecutionIdentity,
}

/// Full wire agent object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    /// `agent_…`.
    pub id: AgentId,
    /// The configuration.
    #[serde(flatten)]
    pub config: AgentConfig,
    /// Archive timestamp (null while active).
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// User metadata.
    pub metadata: Metadata,
    /// Always `"agent"`.
    #[serde(rename = "type")]
    pub tag: AgentTag,
    /// Updated.
    pub updated_at: Timestamp,
    /// Integer version, starts at 1.
    pub version: u64,
}

impl Agent {
    /// The embedded projection used by sessions and threads (live shape:
    /// config, id and version; no timestamps, metadata or archived_at).
    pub fn snapshot(&self) -> AgentSnapshot {
        AgentSnapshot {
            id: self.id.clone(),
            config: self.config.clone(),
            tag: AgentTag::Agent,
            version: self.version,
        }
    }

    /// The compact projection used by deployments and rosters.
    pub fn reference(&self) -> AgentRef {
        AgentRef { id: self.id.clone(), tag: AgentTag::Agent, version: self.version }
    }
}

/// Frozen agent config embedded in sessions and threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Agent id.
    pub id: AgentId,
    /// The configuration.
    #[serde(flatten)]
    pub config: AgentConfig,
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
