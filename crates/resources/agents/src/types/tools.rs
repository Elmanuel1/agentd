//! Tools as stored and served: every config resolved against its toolset's
//! defaults, as live returns them.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Tool permission policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Execute automatically.
    AlwaysAllow,
    /// Pause for `user.tool_confirmation`.
    AlwaysAsk,
    /// The server judges each call: allow when safe, deny when high-risk, ask
    /// when it reaches no judgement.
    Auto,
}

impl PermissionPolicy {
    /// Whether a call under this policy waits for the user's confirmation.
    /// agentd has no call judge, so `auto` reaches no judgement and asks.
    #[must_use]
    pub fn asks(self) -> bool {
        !matches!(self, Self::AlwaysAllow)
    }
}

/// Defaults applied to every tool in a toolset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefaultConfig {
    /// Whether tools are enabled by default.
    pub enabled: bool,
    /// Default permission policy.
    pub permission_policy: PermissionPolicy,
}

impl ToolDefaultConfig {
    /// The agent toolset's default: enabled, `always_allow`.
    pub const AGENT_TOOLSET: Self = Self { enabled: true, permission_policy: PermissionPolicy::AlwaysAllow };
    /// An MCP toolset's default: enabled, `always_ask`.
    pub const MCP_TOOLSET: Self = Self { enabled: true, permission_policy: PermissionPolicy::AlwaysAsk };
}

/// One tool's resolved config inside a toolset. Agent-toolset configs carry
/// `type` (the tool name) and their tool's options (`url_sources` and so on);
/// MCP tool configs carry neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolConfig {
    /// Whether the tool is enabled.
    pub enabled: bool,
    /// Tool name (e.g. `bash`).
    pub name: String,
    /// Permission policy.
    pub permission_policy: PermissionPolicy,
    /// The tool's `type` (agent toolset only).
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Tool-specific options.
    #[serde(flatten)]
    pub options: Map<String, Value>,
}

/// An entry in `agent.tools`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Tool {
    /// The prebuilt toolset (bash/read/write/edit/glob/grep/web_*).
    #[serde(rename = "agent_toolset_20260401")]
    AgentToolset {
        /// Per-tool configs.
        configs: Vec<ToolConfig>,
        /// Toolset defaults.
        default_config: ToolDefaultConfig,
    },
    /// Tools exposed by a declared MCP server.
    #[serde(rename = "mcp_toolset")]
    McpToolset {
        /// Per-tool configs.
        configs: Vec<ToolConfig>,
        /// Defaults.
        default_config: ToolDefaultConfig,
        /// Which `mcp_servers` entry this grants.
        mcp_server_name: String,
    },
    /// A client-side custom tool.
    #[serde(rename = "custom")]
    Custom {
        /// Description shown to the model.
        description: String,
        /// JSON schema of the input.
        input_schema: Value,
        /// Tool name.
        name: String,
    },
}

impl Tool {
    /// The permission policy for a call to agent-toolset tool `name`, when
    /// this entry is the agent toolset.
    pub fn toolset_policy(&self, name: &str) -> Option<PermissionPolicy> {
        match self {
            Self::AgentToolset { configs, default_config } => Some(
                configs
                    .iter()
                    .find(|c| c.name == name)
                    .map_or(default_config.permission_policy, |c| c.permission_policy),
            ),
            Self::McpToolset { .. } | Self::Custom { .. } => None,
        }
    }
}

/// A declared MCP server. It carries no auth: credentials live in vaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
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
