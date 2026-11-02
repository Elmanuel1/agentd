use super::*;

const SESSION_FIXTURE: &str = include_str!("../../../../fixtures/session.create.json");

#[test]
fn fixture_roundtrip_is_lossless() {
    let original: serde_json::Value = serde_json::from_str(SESSION_FIXTURE).unwrap();
    let s: Session = serde_json::from_str(SESSION_FIXTURE).unwrap();
    assert_eq!(serde_json::to_value(&s).unwrap(), original);
}

#[test]
fn agent_ref_input_forms() {
    let a: AgentRefInput = serde_json::from_str("\"agent_01XdReZgC7eHpg9b9Nvrwdcm\"").unwrap();
    assert_eq!(a.id(), "agent_01XdReZgC7eHpg9b9Nvrwdcm");
    assert_eq!(a.version(), None);
    let b: AgentRefInput =
        serde_json::from_str(r#"{"type":"agent","id":"agent_01XdReZgC7eHpg9b9Nvrwdcm","version":3}"#)
            .unwrap();
    assert_eq!(b.version(), Some(3));
}

#[test]
fn repo_resource_token_is_never_serialized() {
    let input: SessionResourceInput = serde_json::from_str(
        r#"{"type":"github_repository","url":"https://github.com/o/r","authorization_token":"ghp_secret"}"#,
    )
    .unwrap();
    // The wire type has no token field at all — constructing it drops it.
    let wire = match input {
        SessionResourceInput::GithubRepository { url, mount_path, checkout, .. } => {
            SessionResource::GithubRepository { id: None, url, mount_path, checkout }
        }
        _ => panic!("wrong variant"),
    };
    let s = serde_json::to_string(&wire).unwrap();
    assert!(!s.contains("ghp_secret"));
    assert!(!s.contains("authorization_token"));
}

#[test]
fn outcome_result_terminality() {
    assert!(OutcomeResult::Satisfied.is_terminal());
    assert!(!OutcomeResult::NeedsRevision.is_terminal());
    assert!(OutcomeResult::MaxIterationsReached.is_terminal());
    assert!(OutcomeResult::Failed.is_terminal());
    assert!(OutcomeResult::Interrupted.is_terminal());
}
