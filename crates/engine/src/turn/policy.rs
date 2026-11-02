//! Which tools an agent offers its model, and what happens when the model
//! calls one: the permission policy is enforced here, in agentd, never in the
//! sandbox.

use std::collections::HashMap;

use agents::types::{PermissionPolicy, Tool};
use agents::AgentSnapshot;
use serde_json::{json, Value};

use super::history::SEND_TO_AGENT;
use crate::model::ModelTool;
use crate::tools::specs;

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
    /// Wait for the client to confirm.
    Ask,
    /// The client runs it (`agent.custom_tool_use`).
    Custom,
    /// A coordinator tool: delegation to the roster.
    Delegate,
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
                // MCP servers are not connected yet; their tools are not offered.
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
/// live evaluates it for toolset tools inside the sandbox, and says so.
fn from_policy(policy: PermissionPolicy) -> Disposition {
    match policy {
        PermissionPolicy::AlwaysAllow => Disposition::Allow { evaluation: None },
        PermissionPolicy::AlwaysAsk => Disposition::Ask,
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

#[cfg(test)]
mod tests {
    use agents::types::{ToolConfig, ToolDefaultConfig};
    use agents::ModelSpec;

    use super::*;

    fn agent(tools: Vec<Tool>) -> AgentSnapshot {
        serde_json::from_value(json!({
            "id": "agent_011111111111111111111111", "type": "agent", "version": 1, "name": "a",
            "model": ModelSpec::standard("claude-haiku-4-5"), "description": null, "system": null,
            "tools": tools, "mcp_servers": [], "skills": [], "multiagent": null,
        }))
        .unwrap()
    }

    #[test]
    fn toolset_configs_decide_what_is_offered_and_how_it_runs() {
        let toolset = Tool::AgentToolset {
            configs: vec![
                serde_json::from_value::<ToolConfig>(json!({"name": "web_fetch", "enabled": false})).unwrap(),
                serde_json::from_value::<ToolConfig>(
                    json!({"name": "bash", "permission_policy": {"type": "always_ask"}}),
                )
                .unwrap(),
            ],
            default_config: ToolDefaultConfig { enabled: true, permission_policy: PermissionPolicy::Auto },
        };
        let custom = Tool::Custom {
            name: "lookup".into(),
            description: "d".into(),
            input_schema: json!({"type": "object"}),
        };
        let b = Toolbox::for_agent(&agent(vec![toolset, custom]), true);
        let names: Vec<&str> = b.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "bash",
                "read",
                "write",
                "edit",
                "glob",
                "grep",
                "web_search",
                "lookup",
                "list_agents",
                "send_to_agent"
            ]
        );
        assert_eq!(b.disposition("bash"), Some(&Disposition::Ask));
        assert!(matches!(b.disposition("read"), Some(Disposition::Allow { evaluation: Some(_) })));
        assert_eq!(b.disposition("lookup"), Some(&Disposition::Custom));
        assert_eq!(b.disposition("web_fetch"), None);
    }
}
