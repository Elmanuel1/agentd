//! Agent wire types — shapes captured live from `POST /v1/agents`.
//!
//! Canonicalization rules (live-verified):
//! - `"model":"claude-sonnet-4-6"` → `{"id":"claude-sonnet-4-6","speed":"standard"}`
//! - `{"type":"agent_toolset_20260401"}` → expanded with
//!   `configs:[]` and `default_config:{enabled:true,permission_policy:{type:"always_allow"}}`;
//!   every tool config is resolved against its toolset's defaults
//! - absent `description`/`system`/`multiagent` → `null`; absent lists → `[]`;
//!   absent `metadata` → `{}`; absent `execution_identity` → the service
//!   account; skill `version` → `latest`; first `version` → `1` (integer).
//!
//! The agent config ([`AgentConfig`]) is written once and flattened into the
//! wire projections: [`Agent`] (full object) and [`AgentSnapshot`] (embedded
//! in sessions and threads, without timestamps or metadata). [`AgentRef`] is
//! the compact `{id,type,version}` used by deployments and rosters.

use serde::{Deserialize, Serialize};
use shared::{Id, Resource};

mod config;
mod identity;
mod model;
mod multiagent;
mod multiagent_params;
mod requests;
mod skills;
mod tool_params;
mod tools;
mod validate;

pub use config::{Agent, AgentConfig, AgentRef, AgentSnapshot};
pub use identity::ExecutionIdentity;
pub use model::{Effort, EffortLevel, ModelInput, ModelSpec, ModelSpeed};
pub use multiagent::{Advisor, AdvisorMember, Members, Multiagent, RosterMember, Toggle};
pub use multiagent_params::{AgentRefParams, MembersParams, MultiagentParams, TaggedRefParams};
pub use requests::{CreateAgentRequest, UpdateAgentRequest};
pub use skills::{SkillParams, SkillRef};
pub use tool_params::{DefaultConfigParams, ToolConfigParams, ToolParams};
pub use tools::{McpServer, PermissionPolicy, Tool, ToolConfig, ToolDefaultConfig};
pub use validate::ConfigDraft;

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

#[cfg(test)]
mod tests;
