//! Which tools an agent offers its model, and what happens when the model
//! calls one: the permission policy is enforced here, in agentd, never in the
//! sandbox.

use std::collections::HashMap;

use agents::AgentSnapshot;
use agents::types::{PermissionPolicy, Tool};
use serde_json::{Value, json};

use super::history::SEND_TO_AGENT;
use super::specs;
use crate::model::ModelTool;

/// The coordinator's roster tool.
pub const LIST_AGENTS: &str = "list_agents";

/// What a call to a tool does.
#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// Run it now; `evaluation` is the live `evaluation` object when a policy
    /// decided automatically.
    Allow {
        /// How the permission was reached.
        evaluation: Option<Value>,
    },
    /// Wait for the client to confirm; `evaluation` names the policy.
    Ask {
        /// How the permission was reached.
        evaluation: Value,
    },
    /// The client runs it (`agent.custom_tool_use`).
    Custom,
    /// A coordinator tool: delegation to the roster.
    Delegate,
    /// A tool of the MCP server `server`, which the loop calls itself.
    Mcp {
        /// The server's name.
        server: String,
        /// The tool's own name, as its events carry it.
        tool: String,
        /// The call waits for the client's confirmation.
        ask: bool,
        /// How the permission was reached.
        evaluation: Option<Value>,
    },
}

/// The tools of one agent.
#[derive(Debug, Clone, Default)]
pub struct Toolbox {
    /// What the model sees.
    pub tools: Vec<ModelTool>,
    dispositions: HashMap<String, Disposition>,
}

impl Toolbox {
    /// The tools `agent` declares; a coordinator also gets its roster tools.
    pub fn for_agent(agent: &AgentSnapshot, coordinator: bool) -> Self {
        let mut b = Self::default();
        for tool in &agent.config.tools {
            match tool {
                Tool::AgentToolset { configs, default_config } => {
                    for name in specs::TOOLSET {
                        let config = configs.iter().find(|c| c.name == name);
                        let enabled = config.map_or(default_config.enabled, |c| c.enabled);
                        let policy = config.map_or(default_config.permission_policy, |c| c.permission_policy);
                        if let (true, Some(spec)) = (enabled, specs::spec(name)) {
                            b.add(spec, from_policy(policy));
                        }
                    }
                }
                Tool::Custom { name, description, input_schema } => b.add(
                    ModelTool {
                        name: name.clone(),
                        description: description.clone(),
                        input_schema: input_schema.clone(),
                    },
                    Disposition::Custom,
                ),
                // Each step adds the tools its servers list (`with_mcp`).
                Tool::McpToolset { .. } => {}
            }
        }
        if coordinator {
            b.add(list_agents(), Disposition::Delegate);
            b.add(send_to_agent(), Disposition::Delegate);
        }
        b
    }

    /// Read-only toolset tools, for a grader.
    pub fn read_only() -> Self {
        let mut b = Self::default();
        for name in specs::READ_ONLY {
            if let Some(spec) = specs::spec(name) {
                b.add(spec, Disposition::Allow { evaluation: None });
            }
        }
        b
    }

    /// Offer `tool` with `disposition`.
    pub fn add(&mut self, tool: ModelTool, disposition: Disposition) {
        self.dispositions.insert(tool.name.clone(), disposition);
        self.tools.push(tool);
    }

    /// What a call to `name` does; `None` for a tool the agent does not have.
    pub fn disposition(&self, name: &str) -> Option<&Disposition> {
        self.dispositions.get(name)
    }
}

/// `always_allow` runs; `always_ask` waits for the client; `auto` runs, as
/// live evaluates it for toolset tools inside the sandbox. Each says so.
pub(super) fn from_policy(policy: PermissionPolicy) -> Disposition {
    match policy {
        PermissionPolicy::AlwaysAllow => {
            Disposition::Allow { evaluation: Some(json!({"type": "always_allow"})) }
        }
        PermissionPolicy::AlwaysAsk => Disposition::Ask { evaluation: json!({"type": "always_ask"}) },
        PermissionPolicy::Auto => Disposition::Allow {
            evaluation: Some(json!({"type": "auto", "evaluated_permission": {"type": "allow"}})),
        },
    }
}

fn list_agents() -> ModelTool {
    ModelTool {
        name: LIST_AGENTS.into(),
        description: "List the agents on your roster, with what each does.".into(),
        input_schema: json!({"type": "object", "properties": {}}),
    }
}

fn send_to_agent() -> ModelTool {
    ModelTool {
        name: SEND_TO_AGENT.into(),
        description: "Send a task or a follow-up to an agent on your roster. Name `agent_name` to start a new thread; name `session_thread_id` to send a follow-up to a thread you started. Returns the thread id at once; the agent's reply arrives later as a message from that thread.".into(),
        input_schema: json!({"type": "object", "properties": {
            "agent_name": {"type": "string", "description": "A roster agent, for a new thread."},
            "session_thread_id": {"type": "string", "description": "A thread you started, for a follow-up."},
            "message": {"type": "string"}},
            "required": ["message"]}),
    }
}

mod mcp;
pub use mcp::model_name;

#[cfg(test)]
mod tests;
