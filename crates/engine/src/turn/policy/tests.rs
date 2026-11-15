//! The toolbox an agent's tools build, and how each call runs.

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
    let asked = Disposition::Ask { evaluation: json!({"type": "always_ask"}) };
    assert_eq!(b.disposition("bash"), Some(&asked));
    assert!(matches!(b.disposition("read"), Some(Disposition::Allow { evaluation: Some(_) })));
    assert_eq!(b.disposition("lookup"), Some(&Disposition::Custom));
    assert_eq!(b.disposition("web_fetch"), None);
}

#[test]
fn a_policy_decision_names_its_policy_as_live_does() {
    let toolset = |policy| Tool::AgentToolset {
        configs: Vec::new(),
        default_config: ToolDefaultConfig { enabled: true, permission_policy: policy },
    };
    let allowed = Toolbox::for_agent(&agent(vec![toolset(PermissionPolicy::AlwaysAllow)]), false);
    let evaluation = Some(json!({"type": "always_allow"}));
    assert_eq!(allowed.disposition("bash"), Some(&Disposition::Allow { evaluation }));
}
