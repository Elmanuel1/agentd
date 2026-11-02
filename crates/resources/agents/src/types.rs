//! Agent wire types — shapes captured live from `POST /v1/agents`.
//!
//! Canonicalization rules (live-verified):
//! - `"model":"claude-sonnet-4-6"` → `{"id":"claude-sonnet-4-6","speed":"standard"}`
//! - `{"type":"agent_toolset_20260401"}` → expanded with
//!   `configs:[]` and `default_config:{enabled:true,permission_policy:{type:"always_allow"}}`
//! - absent `description`/`system`/`multiagent` → `null`; absent lists → `[]`;
//!   absent `metadata` → `{}`; first `version` → `1` (integer).
//!
//! The agent config appears in three wire projections:
//! [`Agent`] (full object), [`AgentSnapshot`] (embedded in sessions/threads —
//! no timestamps/metadata), [`AgentRef`] (compact `{id,type,version}` in
//! deployments).

use serde::{Deserialize, Serialize};
use shared::{Id, Resource};

mod config;
mod multiagent;
mod requests;
mod skills;
mod tools;

pub use config::{Agent, AgentConfig, AgentRef, AgentSnapshot};
pub use multiagent::{Multiagent, RosterEntry, TaggedRosterEntry};
pub use requests::{CreateAgentRequest, UpdateAgentRequest};
pub use skills::SkillRef;
pub use tools::{McpServer, PermissionPolicy, Tool, ToolConfig, ToolDefaultConfig};

/// Marker type for agent IDs (`agent_…`).
#[derive(Debug, Clone, Copy)]
pub struct AgentResource;
impl Resource for AgentResource {
    const PREFIX: &'static str = "agent";
}
/// `Id<AgentResource>` alias.
pub type AgentId = Id<AgentResource>;

/// Constant `"type":"agent"` discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTag {
    /// Always `agent`.
    Agent,
}

/// Model execution speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSpeed {
    /// Default.
    Standard,
    /// Fast mode (Opus 4.6 only, per upstream).
    Fast,
}

/// Canonical model object: `{"id":"…","speed":"standard"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSpec {
    /// Model id.
    pub id: String,
    /// Speed tier.
    pub speed: ModelSpeed,
}

/// Accepts the wire's model shorthand: bare string or full object.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ModelInput {
    /// `"claude-sonnet-4-6"`.
    Shorthand(String),
    /// `{"id":"…","speed":"…"}`.
    Full {
        /// Model id.
        id: String,
        /// Optional speed (defaults to standard).
        #[serde(default)]
        speed: Option<ModelSpeed>,
    },
}

impl ModelInput {
    /// Canonicalize to the full object (live rule: shorthand → standard speed).
    pub fn canonicalize(self) -> ModelSpec {
        match self {
            Self::Shorthand(id) => ModelSpec { id, speed: ModelSpeed::Standard },
            Self::Full { id, speed } => ModelSpec { id, speed: speed.unwrap_or(ModelSpeed::Standard) },
        }
    }
}

#[cfg(test)]
mod tests;
