//! Tools as a request sends them: configs and defaults may be partial or
//! `null`, and are resolved into [`Tool`] before they are stored.

use serde::Deserialize;
use serde_json::{Map, Value};
use shared::ApiError;

use super::tools::{PermissionPolicy, Tool, ToolConfig, ToolDefaultConfig};

/// The tools of the agent toolset, and the options each one accepts.
const AGENT_TOOLS: &[(&str, &[&str])] = &[
    ("bash", &[]),
    ("edit", &[]),
    ("read", &[]),
    ("write", &[]),
    ("glob", &[]),
    ("grep", &[]),
    ("web_fetch", &["allowed_domains", "blocked_domains", "max_content_tokens", "url_sources"]),
    ("web_search", &["allowed_domains", "blocked_domains", "user_location"]),
];

/// The `url_sources` members; each may be a shorthand (`"all"`, `"none"`) or
/// an object, and a member left out is `null`.
const URL_SOURCES: &[&str] = &["client_tool_results", "server_tool_results", "user_input"];

/// Partial toolset defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultConfigParams {
    /// Enabled, `true` by default.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Policy, the toolset's default when left out.
    #[serde(default)]
    pub permission_policy: Option<PermissionPolicy>,
}

/// One tool's partial config.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolConfigParams {
    /// Tool name.
    pub name: String,
    /// Enabled; the toolset default when left out.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Policy; the toolset default when left out.
    #[serde(default)]
    pub permission_policy: Option<PermissionPolicy>,
    /// The tool's `type`, which must equal its name.
    #[serde(rename = "type", default)]
    pub tag: Option<String>,
    /// Tool-specific options.
    #[serde(flatten)]
    pub options: Map<String, Value>,
}

/// An entry in a request's `tools`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ToolParams {
    /// The prebuilt toolset.
    #[serde(rename = "agent_toolset_20260401")]
    AgentToolset {
        /// Per-tool configs.
        #[serde(default)]
        configs: Vec<ToolConfigParams>,
        /// Defaults.
        #[serde(default)]
        default_config: Option<DefaultConfigParams>,
    },
    /// An MCP server's tools.
    #[serde(rename = "mcp_toolset")]
    McpToolset {
        /// Which `mcp_servers` entry this grants.
        mcp_server_name: String,
        /// Per-tool configs.
        #[serde(default)]
        configs: Vec<ToolConfigParams>,
        /// Defaults.
        #[serde(default)]
        default_config: Option<DefaultConfigParams>,
    },
    /// A client-side custom tool.
    #[serde(rename = "custom")]
    Custom {
        /// Tool name.
        name: String,
        /// Description shown to the model.
        #[serde(default)]
        description: Option<String>,
        /// JSON schema of the input.
        input_schema: Value,
    },
}

impl ToolParams {
    /// The stored form: defaults filled in, every config resolved against them.
    pub fn resolve(self) -> Result<Tool, ApiError> {
        Ok(match self {
            Self::AgentToolset { configs, default_config } => {
                let defaults = defaults(default_config, ToolDefaultConfig::AGENT_TOOLSET);
                let configs =
                    configs.into_iter().map(|c| agent_tool(c, &defaults)).collect::<Result<_, _>>()?;
                Tool::AgentToolset { configs, default_config: defaults }
            }
            Self::McpToolset { mcp_server_name, configs, default_config } => {
                let defaults = defaults(default_config, ToolDefaultConfig::MCP_TOOLSET);
                let configs =
                    configs.into_iter().map(|c| mcp_tool(c, &defaults)).collect::<Result<_, _>>()?;
                Tool::McpToolset { configs, default_config: defaults, mcp_server_name }
            }
            Self::Custom { name, description, input_schema } => {
                Tool::Custom { description: description.unwrap_or_default(), input_schema, name }
            }
        })
    }
}

fn defaults(params: Option<DefaultConfigParams>, base: ToolDefaultConfig) -> ToolDefaultConfig {
    let p = params.unwrap_or_default();
    ToolDefaultConfig {
        enabled: p.enabled.unwrap_or(base.enabled),
        permission_policy: p.permission_policy.unwrap_or(base.permission_policy),
    }
}

fn unknown_field(name: &str) -> ApiError {
    ApiError::invalid_request(format!("Failed to parse request body: unknown field \"{name}\""))
}

fn agent_tool(c: ToolConfigParams, defaults: &ToolDefaultConfig) -> Result<ToolConfig, ApiError> {
    let Some((name, allowed)) = AGENT_TOOLS.iter().find(|(n, _)| *n == c.name) else {
        return Err(ApiError::invalid_request(format!("Unknown tool name in configs: {}", c.name)));
    };
    if c.tag.as_deref().is_some_and(|t| t != *name) {
        return Err(ApiError::invalid_request(format!("Tool config type must match its name: {name}")));
    }
    if let Some(key) = c.options.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(unknown_field(key));
    }
    let mut options = c.options;
    if *name == "web_fetch" {
        let sources = options.remove("url_sources").unwrap_or(Value::Null);
        options.insert("url_sources".into(), url_sources(sources)?);
    }
    Ok(ToolConfig {
        enabled: c.enabled.unwrap_or(defaults.enabled),
        name: (*name).to_owned(),
        permission_policy: c.permission_policy.unwrap_or(defaults.permission_policy),
        tag: Some((*name).to_owned()),
        options,
    })
}

fn mcp_tool(c: ToolConfigParams, defaults: &ToolDefaultConfig) -> Result<ToolConfig, ApiError> {
    if c.tag.is_some() {
        return Err(unknown_field("type"));
    }
    if let Some(key) = c.options.keys().next() {
        return Err(unknown_field(key));
    }
    Ok(ToolConfig {
        enabled: c.enabled.unwrap_or(defaults.enabled),
        name: c.name,
        permission_policy: c.permission_policy.unwrap_or(defaults.permission_policy),
        tag: None,
        options: Map::new(),
    })
}

/// `url_sources` with every member present, shorthands expanded to objects.
fn url_sources(sources: Value) -> Result<Value, ApiError> {
    let mut members = match sources {
        Value::Null => return Ok(Value::Null),
        Value::Object(m) => m,
        _ => return Err(ApiError::invalid_request("url_sources: Input should be an object")),
    };
    if let Some(key) = members.keys().find(|k| !URL_SOURCES.contains(&k.as_str())) {
        return Err(unknown_field(key));
    }
    let mut resolved = Map::new();
    for key in URL_SOURCES {
        let member = match members.remove(*key) {
            Some(Value::String(shorthand)) => serde_json::json!({ "type": shorthand }),
            Some(other) => other,
            None => Value::Null,
        };
        resolved.insert((*key).to_owned(), member);
    }
    Ok(Value::Object(resolved))
}
