use super::*;

const AGENT_FIXTURE: &str = include_str!("../../../../fixtures/agent.create.json");

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
    let m: ModelInput = serde_json::from_str(r#"{"id":"claude-opus-4-6","speed":"fast"}"#).unwrap();
    assert_eq!(m.canonicalize(), ModelSpec { id: "claude-opus-4-6".into(), speed: ModelSpeed::Fast });
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
    assert_eq!(v["default_config"]["permission_policy"]["type"], "always_allow");
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
    assert!(matches!(roster[1], RosterEntry::Tagged(TaggedRosterEntry::Agent { version: Some(4), .. })));
    assert!(matches!(roster[2], RosterEntry::Tagged(TaggedRosterEntry::SelfEntry {})));
}

#[test]
fn create_request_is_permissive_about_missing_fields() {
    // Missing model must deserialize fine — the service produces the
    // exact "model: Field required" message, not serde.
    let r: CreateAgentRequest = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
    assert!(r.model.is_none());
    assert_eq!(r.name.as_deref(), Some("x"));
}
