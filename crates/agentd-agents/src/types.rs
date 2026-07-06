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

use agentd_core::{Id, Metadata, Resource, Timestamp};
use serde::{Deserialize, Serialize};

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

// ─── Model ─────────────────────────────────────────────────────────────────

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
            Self::Shorthand(id) => ModelSpec {
                id,
                speed: ModelSpeed::Standard,
            },
            Self::Full { id, speed } => ModelSpec {
                id,
                speed: speed.unwrap_or(ModelSpeed::Standard),
            },
        }
    }
}

// ─── Tools ─────────────────────────────────────────────────────────────────

/// Tool permission policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Execute automatically (default).
    AlwaysAllow,
    /// Pause for `user.tool_confirmation`.
    AlwaysAsk,
}

/// Defaults applied to all tools in a toolset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefaultConfig {
    /// Whether tools are enabled by default.
    pub enabled: bool,
    /// Default permission policy.
    pub permission_policy: PermissionPolicy,
}

impl Default for ToolDefaultConfig {
    fn default() -> Self {
        // Live canonical expansion of a bare toolset entry.
        Self {
            enabled: true,
            permission_policy: PermissionPolicy::AlwaysAllow,
        }
    }
}

/// Per-tool override inside a toolset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Tool name (e.g. `bash`).
    pub name: String,
    /// Enabled override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Permission override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_policy: Option<PermissionPolicy>,
}

/// An entry in `agent.tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Tool {
    /// The prebuilt toolset (bash/read/write/edit/glob/grep/web_*).
    #[serde(rename = "agent_toolset_20260401")]
    AgentToolset {
        /// Per-tool overrides (canonical: present, possibly empty).
        #[serde(default)]
        configs: Vec<ToolConfig>,
        /// Toolset defaults (canonical: always present).
        #[serde(default)]
        default_config: ToolDefaultConfig,
    },
    /// Tools exposed by a declared MCP server.
    #[serde(rename = "mcp_toolset")]
    McpToolset {
        /// Which `mcp_servers` entry this grants.
        mcp_server_name: String,
        /// Per-tool overrides.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        configs: Vec<ToolConfig>,
        /// Defaults.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default_config: Option<ToolDefaultConfig>,
    },
    /// A client-side custom tool.
    #[serde(rename = "custom")]
    Custom {
        /// Tool name.
        name: String,
        /// Description shown to the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        /// JSON schema of the input.
        input_schema: serde_json::Value,
    },
}

// ─── MCP servers / skills / multiagent ─────────────────────────────────────

/// A declared MCP server (no auth here — credentials live in vaults).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpServer {
    /// Streamable-HTTP MCP server by URL.
    Url {
        /// Unique name, referenced by `mcp_toolset.mcp_server_name`.
        name: String,
        /// Endpoint URL.
        url: String,
    },
}

impl McpServer {
    /// The unique server name.
    pub fn name(&self) -> &str {
        match self {
            Self::Url { name, .. } => name,
        }
    }
}

/// A skill reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillRef {
    /// Anthropic prebuilt skill (`xlsx`, `pdf`, …).
    Anthropic {
        /// Skill id.
        skill_id: String,
    },
    /// Custom skill from the Skills API.
    Custom {
        /// `skill_…` id.
        skill_id: String,
        /// Version (`"latest"` or a number-as-string).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
}

impl SkillRef {
    /// The skill id (dup-checked at validation).
    pub fn skill_id(&self) -> &str {
        match self {
            Self::Anthropic { skill_id } | Self::Custom { skill_id, .. } => skill_id,
        }
    }
}

/// Multiagent roster entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RosterEntry {
    /// Bare agent id string → latest version.
    Shorthand(String),
    /// Tagged reference.
    Tagged(TaggedRosterEntry),
}

/// Tagged roster entry forms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaggedRosterEntry {
    /// A stored agent, optionally pinned.
    Agent {
        /// Agent id.
        id: String,
        /// Pinned version.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u64>,
    },
    /// The coordinator itself.
    #[serde(rename = "self")]
    SelfEntry {},
}

/// Coordinator config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Multiagent {
    /// This agent may delegate to a roster.
    Coordinator {
        /// 1–20 roster entries.
        agents: Vec<RosterEntry>,
    },
}

// ─── Config + projections ──────────────────────────────────────────────────

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
        AgentRef {
            id: self.id.clone(),
            tag: AgentTag::Agent,
            version: self.version,
        }
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

// ─── Requests ──────────────────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT_FIXTURE: &str = include_str!("../../../fixtures/agent.create.json");

    #[test]
    fn fixture_roundtrip_is_lossless() {
        // Deserialize the live capture into Agent, re-serialize, and compare
        // structurally — proves our types cover every field with the right
        // shapes and null/[] conventions.
        let original: serde_json::Value = serde_json::from_str(AGENT_FIXTURE).unwrap();
        let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
        let ours = serde_json::to_value(&agent).unwrap();
        assert_eq!(ours, original);
    }

    #[test]
    fn model_shorthand_canonicalizes_to_standard_speed() {
        // Live rule: "claude-sonnet-4-6" → {"id":…,"speed":"standard"}
        let m: ModelInput = serde_json::from_str("\"claude-sonnet-4-6\"").unwrap();
        let spec = m.canonicalize();
        assert_eq!(
            serde_json::to_value(&spec).unwrap(),
            serde_json::json!({"id":"claude-sonnet-4-6","speed":"standard"})
        );
    }

    #[test]
    fn model_object_form_accepted() {
        let m: ModelInput =
            serde_json::from_str(r#"{"id":"claude-opus-4-6","speed":"fast"}"#).unwrap();
        assert_eq!(
            m.canonicalize(),
            ModelSpec {
                id: "claude-opus-4-6".into(),
                speed: ModelSpeed::Fast
            }
        );
    }

    #[test]
    fn bare_toolset_expands_to_live_canonical_form() {
        // Input {"type":"agent_toolset_20260401"} must expand to the fixture's
        // {configs:[], default_config:{enabled:true, permission_policy:{type:"always_allow"}}}
        let t: Tool = serde_json::from_str(r#"{"type":"agent_toolset_20260401"}"#).unwrap();
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["type"], "agent_toolset_20260401");
        assert_eq!(v["configs"], serde_json::json!([]));
        assert_eq!(v["default_config"]["enabled"], true);
        assert_eq!(
            v["default_config"]["permission_policy"]["type"],
            "always_allow"
        );
    }

    #[test]
    fn snapshot_matches_live_session_embedding() {
        // Live session fixture embeds the agent WITHOUT timestamps/metadata.
        let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
        let snap = serde_json::to_value(agent.snapshot()).unwrap();
        let keys: Vec<&str> = snap.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "description",
                "id",
                "mcp_servers",
                "model",
                "multiagent",
                "name",
                "skills",
                "system",
                "tools",
                "type",
                "version"
            ]
        );
    }

    #[test]
    fn agent_ref_matches_live_deployment_embedding() {
        // Live deployment fixture: "agent":{"id":"…","type":"agent","version":1}
        let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
        let r = serde_json::to_value(agent.reference()).unwrap();
        assert_eq!(
            r,
            serde_json::json!({
                "id": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
                "type": "agent",
                "version": 1
            })
        );
    }

    #[test]
    fn roster_entries_accept_all_three_forms() {
        let roster: Vec<RosterEntry> = serde_json::from_str(
            r#"["agent_01XdReZgC7eHpg9b9Nvrwdcm",
                {"type":"agent","id":"agent_01SW8QNzJHFPdZf76tukfWPu","version":4},
                {"type":"self"}]"#,
        )
        .unwrap();
        assert_eq!(roster.len(), 3);
        assert!(matches!(roster[0], RosterEntry::Shorthand(_)));
        assert!(matches!(
            roster[1],
            RosterEntry::Tagged(TaggedRosterEntry::Agent {
                version: Some(4),
                ..
            })
        ));
        assert!(matches!(
            roster[2],
            RosterEntry::Tagged(TaggedRosterEntry::SelfEntry {})
        ));
    }

    #[test]
    fn create_request_is_permissive_about_missing_fields() {
        // Missing model must deserialize fine — the service produces the
        // exact "model: Field required" message, not serde.
        let r: CreateAgentRequest = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert!(r.model.is_none());
        assert_eq!(r.name.as_deref(), Some("x"));
    }
}
