use super::*;

#[test]
fn secret_string_debug_is_redacted() {
    let s = SecretString::new("xoxp-super-secret");
    assert_eq!(format!("{s:?}"), "SecretString(***)");
    assert_eq!(s.expose(), "xoxp-super-secret");
}

#[test]
fn wire_projection_carries_no_secret_material() {
    let input: CredentialAuthInput = serde_json::from_str(
        r#"{"type":"mcp_oauth","mcp_server_url":"https://mcp.slack.com/mcp",
            "access_token":"xoxp-secret","expires_at":"2026-04-15T00:00:00.000000Z",
            "refresh":{"token_endpoint":"https://slack.com/api/oauth.v2.access",
                       "client_id":"123.456","refresh_token":"xoxe-1-secret",
                       "scope":"channels:read",
                       "token_endpoint_auth":{"type":"client_secret_post","client_secret":"abc"}}}"#,
    )
    .unwrap();
    let wire = input.to_wire();
    let v = serde_json::to_value(&wire).unwrap();
    let raw = v.to_string();
    // No secret substring can appear in the serialized wire form.
    assert!(!raw.contains("xoxp-secret"));
    assert!(!raw.contains("xoxe-1-secret"));
    assert!(!raw.contains("abc"));
    assert_eq!(v["type"], "mcp_oauth");
    assert_eq!(v["mcp_server_url"], "https://mcp.slack.com/mcp");
    assert_eq!(v["has_refresh"], true);
}

#[test]
fn unique_keys_by_variant() {
    let oauth: CredentialAuthInput =
        serde_json::from_str(r#"{"type":"mcp_oauth","mcp_server_url":"https://a/mcp","access_token":"t"}"#)
            .unwrap();
    assert_eq!(oauth.unique_key(), "https://a/mcp");
    let env: CredentialAuthInput = serde_json::from_str(
        r#"{"type":"environment_variable","secret_name":"TWILIO_API_KEY","secret_value":"sk","networking":{"type":"unrestricted"}}"#,
    )
    .unwrap();
    assert_eq!(env.unique_key(), "TWILIO_API_KEY");
}

#[test]
fn env_var_networking_shapes() {
    let n: CredentialNetworking =
        serde_json::from_str(r#"{"type":"limited","allowed_hosts":["api.twilio.com","*.twilio.com"]}"#)
            .unwrap();
    assert!(matches!(n, CredentialNetworking::Limited { .. }));
}
