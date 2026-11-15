use super::*;

struct AgentRes;
impl Resource for AgentRes {
    const PREFIX: &'static str = "agent";
}
struct SessionRes;
impl Resource for SessionRes {
    const PREFIX: &'static str = "sesn";
    const LABEL: &'static str = "session";
}

#[test]
fn generated_id_matches_wire_grammar() {
    let id = Id::<AgentRes>::generate();
    let s = id.as_str();
    assert!(s.starts_with("agent_01"), "got {s}");
    let body = &s["agent_".len()..];
    assert_eq!(body.len(), 24);
    assert!(body.bytes().all(|b| b.is_ascii_alphanumeric()));
}

#[test]
fn a_derived_id_is_the_same_for_its_seed_and_parses() {
    let a = Id::<AgentRes>::derived("depl_1@2026-11-12T03:00:00Z");
    assert_eq!(a, Id::<AgentRes>::derived("depl_1@2026-11-12T03:00:00Z"));
    assert_ne!(a, Id::<AgentRes>::derived("depl_1@2026-11-13T03:00:00Z"));
    assert_eq!(Id::<AgentRes>::parse(a.as_str()).unwrap(), a);
    assert_eq!(a.as_str().len(), "agent_".len() + 24);
}

#[test]
fn parse_accepts_live_captured_ids() {
    // Real IDs captured from the live API.
    for raw in ["agent_01XdReZgC7eHpg9b9Nvrwdcm", "agent_01SW8QNzJHFPdZf76tukfWPu"] {
        assert!(Id::<AgentRes>::parse(raw).is_ok(), "{raw}");
    }
}

#[test]
fn parse_rejects_wrong_prefix() {
    assert!(Id::<AgentRes>::parse("sesn_01KZqwuX11cmU6RPvKfze8rR").is_err());
}

#[test]
fn parse_rejects_short_body_like_live_api() {
    // Live probe: GET /v1/agents/agent_00bogus → 400 "Invalid agent ID."
    let err = Id::<AgentRes>::parse("agent_00bogus").unwrap_err();
    assert_eq!(err.to_string(), "Invalid agent ID.");
}

#[test]
fn label_overrides_error_message() {
    let err = Id::<SessionRes>::parse("sesn_x").unwrap_err();
    assert_eq!(err.to_string(), "Invalid session ID.");
}

#[test]
fn parse_rejects_non_alphanumeric() {
    assert!(Id::<AgentRes>::parse("agent_01XdReZgC7eHpg9b9Nvrw-cm").is_err());
}

#[test]
fn serde_roundtrip() {
    let id = Id::<AgentRes>::generate();
    let json = serde_json::to_string(&id).unwrap();
    let back: Id<AgentRes> = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}

#[test]
fn deserialize_rejects_malformed() {
    let r: Result<Id<AgentRes>, _> = serde_json::from_str("\"agent_00bogus\"");
    assert!(r.is_err());
}

#[test]
fn request_id_grammar() {
    let r = RequestId::generate();
    assert!(r.as_str().starts_with("req_01"));
    assert_eq!(r.as_str().len(), "req_".len() + 24);
}

struct RenamedRes;
impl Resource for RenamedRes {
    const PREFIX: &'static str = "vcrd";
    const EARLIER_PREFIXES: &'static [&'static str] = &["cred"];
}

#[test]
fn a_new_id_takes_the_current_prefix_and_an_earlier_one_still_parses() {
    assert!(Id::<RenamedRes>::generate().as_str().starts_with("vcrd_01"));
    assert!(Id::<RenamedRes>::parse("cred_01AAAAAAAAAAAAAAAAAAAAAA").is_ok());
    assert!(Id::<RenamedRes>::parse("other_01AAAAAAAAAAAAAAAAAAAAAA").is_err());
}
