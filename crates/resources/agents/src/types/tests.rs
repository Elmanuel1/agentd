use super::*;
use shared::limits::ModelCatalog;

const AGENT_FIXTURE: &str = include_str!("../../../../../conformance/fixtures/agent.create.json");

fn fixture() -> serde_json::Value {
    serde_json::from_str(AGENT_FIXTURE).unwrap()
}

#[test]
fn fixture_roundtrip_adds_only_the_newer_fields() {
    // The July capture predates execution_identity; everything else
    // round-trips unchanged.
    let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
    let mut expected = fixture();
    expected["execution_identity"] = serde_json::json!({"type": "service_account"});
    assert_eq!(serde_json::to_value(&agent).unwrap(), expected);
}

#[test]
fn model_shorthand_canonicalizes_to_standard_speed() {
    let m: ModelInput = serde_json::from_str("\"claude-sonnet-4-6\"").unwrap();
    assert_eq!(
        serde_json::to_value(m.canonicalize()).unwrap(),
        serde_json::json!({"id":"claude-sonnet-4-6","speed":"standard"})
    );
}

#[test]
fn model_object_keeps_effort_and_inference_geo() {
    let m: ModelInput = serde_json::from_str(
        r#"{"id":"claude-opus-4-6","speed":"fast","effort":"high","inference_geo":"us"}"#,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(m.canonicalize()).unwrap(),
        serde_json::json!({"id":"claude-opus-4-6","speed":"fast","effort":{"type":"high"},"inference_geo":"us"})
    );
}

fn resolve(json: &str) -> serde_json::Value {
    let t: ToolParams = serde_json::from_str(json).unwrap();
    serde_json::to_value(t.resolve().unwrap()).unwrap()
}

#[test]
fn a_bare_toolset_expands_to_the_live_canonical_form() {
    assert_eq!(
        resolve(r#"{"type":"agent_toolset_20260401"}"#),
        serde_json::json!({"type":"agent_toolset_20260401","configs":[],
            "default_config":{"enabled":true,"permission_policy":{"type":"always_allow"}}})
    );
}

#[test]
fn tool_configs_resolve_against_partial_defaults() {
    let v = resolve(
        r#"{"type":"agent_toolset_20260401",
            "default_config":{"permission_policy":{"type":"auto"}},
            "configs":[{"name":"bash","enabled":false},{"name":"web_fetch","url_sources":{"user_input":"all"}}]}"#,
    );
    assert_eq!(v["default_config"], serde_json::json!({"enabled":true,"permission_policy":{"type":"auto"}}));
    assert_eq!(
        v["configs"][0],
        serde_json::json!({"enabled":false,"name":"bash","permission_policy":{"type":"auto"},"type":"bash"})
    );
    assert_eq!(
        v["configs"][1]["url_sources"],
        serde_json::json!({"client_tool_results":null,"server_tool_results":null,"user_input":{"type":"all"}})
    );
}

#[test]
fn an_mcp_toolset_always_carries_configs_and_default_config() {
    let v = resolve(r#"{"type":"mcp_toolset","mcp_server_name":"gh","default_config":null}"#);
    assert_eq!(v["configs"], serde_json::json!([]));
    assert_eq!(
        v["default_config"],
        serde_json::json!({"enabled":true,"permission_policy":{"type":"always_ask"}})
    );
}

#[test]
fn unknown_tool_options_are_rejected() {
    let t: ToolParams = serde_json::from_str(
        r#"{"type":"agent_toolset_20260401","configs":[{"name":"bash","url_sources":null}]}"#,
    )
    .unwrap();
    assert_eq!(
        t.resolve().unwrap_err().message,
        "Failed to parse request body: unknown field \"url_sources\""
    );
}

#[test]
fn a_custom_tool_always_has_a_description() {
    let v = resolve(r#"{"type":"custom","name":"lookup","input_schema":{"type":"object"}}"#);
    assert_eq!(v["description"], "");
}

#[test]
fn snapshot_matches_live_session_embedding() {
    // Live session fixture embeds the agent WITHOUT timestamps/metadata.
    let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
    let snap = serde_json::to_value(agent.snapshot()).unwrap();
    let mut keys: Vec<&str> = snap.as_object().unwrap().keys().map(|s| s.as_str()).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "description",
            "execution_identity",
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
    let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
    assert_eq!(
        serde_json::to_value(agent.reference()).unwrap(),
        serde_json::json!({"id": "agent_01XdReZgC7eHpg9b9Nvrwdcm", "type": "agent", "version": 1})
    );
}

#[test]
fn roster_entries_accept_every_sdk_form() {
    let roster: Vec<AgentRefParams> = serde_json::from_str(
        r#"["agent_01XdReZgC7eHpg9b9Nvrwdcm",
            {"type":"agent","id":"agent_01SW8QNzJHFPdZf76tukfWPu","version":4},
            {"type":"self"},
            {"type":"advisor","model":"claude-opus-4-6"}]"#,
    )
    .unwrap();
    assert!(matches!(roster[0], AgentRefParams::Id(_)));
    assert!(matches!(roster[1], AgentRefParams::Tagged(TaggedRefParams::Agent { version: Some(4), .. })));
    assert!(matches!(roster[2], AgentRefParams::Tagged(TaggedRefParams::SelfRef {})));
    assert!(matches!(roster[3], AgentRefParams::Tagged(TaggedRefParams::Advisor { .. })));
}

#[test]
fn skills_default_to_the_latest_version() {
    let s: SkillParams = serde_json::from_str(r#"{"type":"anthropic","skill_id":"xlsx"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(s.resolve()).unwrap(),
        serde_json::json!({"type":"anthropic","skill_id":"xlsx","version":"latest"})
    );
}

#[test]
fn update_clears_on_null_and_merges_metadata() {
    let agent: Agent = serde_json::from_str(AGENT_FIXTURE).unwrap();
    let req: UpdateAgentRequest = serde_json::from_str(
        r#"{"description":null,"tools":null,"metadata":{"k":"v"},"execution_identity":null}"#,
    )
    .unwrap();
    let draft = req.apply_to(&agent, &ModelCatalog::default_catalog()).unwrap();
    assert_eq!(draft.config.description, None);
    assert!(draft.config.tools.is_empty());
    assert_eq!(draft.metadata, shared::Metadata::from([("k", "v")]));
    assert_eq!(draft.config.execution_identity, ExecutionIdentity::ServiceAccount);
    assert_eq!(draft.config.name, agent.config.name);
}

#[test]
fn create_request_is_permissive_about_missing_fields() {
    // Missing model must deserialize fine — the service produces the
    // exact "model: Field required" message, not serde.
    let r: CreateAgentRequest = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
    assert!(r.model.is_none());
    assert_eq!(r.name.as_deref(), Some("x"));
}

#[test]
fn docs_stored_before_configs_were_resolved_still_read() {
    let t: Tool = serde_json::from_str(r#"{"type":"mcp_toolset","mcp_server_name":"gh"}"#).unwrap();
    assert!(matches!(t, Tool::McpToolset { ref configs, ref default_config, .. }
        if configs.is_empty() && *default_config == ToolDefaultConfig::MCP_TOOLSET));
    let t: Tool = serde_json::from_str(
        r#"{"type":"agent_toolset_20260401","configs":[{"name":"bash"}],
        "default_config":{"enabled":true,"permission_policy":{"type":"always_allow"}}}"#,
    )
    .unwrap();
    assert!(matches!(t, Tool::AgentToolset { ref configs, .. } if configs[0].enabled));
}
