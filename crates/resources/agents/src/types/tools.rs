use serde::{Deserialize, Serialize};

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
        Self { enabled: true, permission_policy: PermissionPolicy::AlwaysAllow }
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
