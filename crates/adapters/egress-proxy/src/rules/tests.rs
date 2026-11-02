use super::*;

fn github() -> Binding {
    Binding {
        id: "vcrd_1".into(),
        name: "GITHUB_TOKEN".into(),
        placeholder: "agentd-ph-abc".into(),
        key: SecretKey::new("wrkspc_1", "vlt_1", "vcrd_1").unwrap(),
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
fn a_placeholder_is_found_wherever_it_sits() {
    assert!(github().found_in(b"Bearer agentd-ph-abc"));
    assert!(github().found_in(b"agentd-ph-abc"));
    assert!(!github().found_in(b"agentd-ph-ab"));
}

#[test]
fn replace_swaps_every_occurrence_or_says_there_is_none() {
    assert_eq!(replace(b"a X b X", b"X", b"yy").unwrap(), b"a yy b yy");
    assert_eq!(replace(b"nothing", b"X", b"yy"), None);
    assert_eq!(replace(b"x", b"", b"yy"), None);
}
