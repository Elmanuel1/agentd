use http::{HeaderMap, HeaderValue};

use super::*;

fn github() -> Credential {
    Credential {
        id: "vcrd_1".into(),
        name: "GITHUB_TOKEN".into(),
        placeholder: "agentd-ph-abc".into(),
        value: "ghp_real".into(),
        hosts: Some(vec!["api.github.com".into(), "*.githubusercontent.com".into()]),
        header: true,
    }
}

#[test]
fn hosts_match_as_documented() {
    assert!(host_matches("example.com", "EXAMPLE.com"));
    assert!(!host_matches("example.com", "www.example.com"));
    assert!(host_matches("*.example.com", "a.b.example.com"));
    assert!(!host_matches("*.example.com", "example.com"));
    assert!(!host_matches("*.example.com", "badexample.com"));
}

#[test]
fn a_placeholder_becomes_the_secret_for_an_allowed_host() {
    let mut h = HeaderMap::new();
    h.insert("authorization", HeaderValue::from_static("Bearer agentd-ph-abc"));
    assert_eq!(inject(&mut h, &[github()], "api.github.com"), Ok(vec!["vcrd_1".to_owned()]));
    assert_eq!(h["authorization"], "Bearer ghp_real");
}

#[test]
fn a_placeholder_for_another_host_is_refused() {
    let mut h = HeaderMap::new();
    h.insert("x-token", HeaderValue::from_static("agentd-ph-abc"));
    let refused = inject(&mut h, &[github()], "evil.example");
    assert_eq!(refused, Err(Refusal::PlaceholderToWrongHost { name: "GITHUB_TOKEN".into() }));
    assert_eq!(h["x-token"], "agentd-ph-abc");
}

#[test]
fn a_request_without_placeholders_is_left_alone() {
    let mut h = HeaderMap::new();
    h.insert("accept", HeaderValue::from_static("*/*"));
    assert_eq!(inject(&mut h, &[github()], "evil.example"), Ok(vec![]));
}

#[test]
fn a_response_echoing_the_secret_gets_the_placeholder_back() {
    let (body, found) = redact(b"{\"token\":\"ghp_real\",\"again\":\"ghp_real\"}", &[github()]);
    assert_eq!(body, b"{\"token\":\"agentd-ph-abc\",\"again\":\"agentd-ph-abc\"}");
    assert_eq!(found, ["vcrd_1"]);
    assert_eq!(redact(b"nothing", &[github()]).1, Vec::<String>::new());
}

#[test]
fn debug_never_prints_the_value() {
    assert!(!format!("{:?}", github()).contains("ghp_real"));
}

#[test]
fn a_credential_without_allowed_hosts_is_sent_nowhere() {
    let mut c = github();
    c.hosts = None;
    let mut h = HeaderMap::new();
    h.insert("authorization", HeaderValue::from_static("Bearer agentd-ph-abc"));
    assert!(matches!(inject(&mut h, &[c], "api.github.com"), Err(Refusal::PlaceholderToWrongHost { .. })));
}
