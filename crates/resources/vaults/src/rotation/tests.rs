//! Live's rotation rules (probed 2026-11-15), one per test.

use super::*;
use crate::types::SecretString;

fn auth(json: &str) -> CredentialAuthInput {
    serde_json::from_str(json).unwrap()
}

fn update(json: &str) -> CredentialAuthUpdate {
    serde_json::from_str(json).unwrap()
}

const OAUTH: &str = r#"{"type":"mcp_oauth","mcp_server_url":"https://a/mcp","access_token":"t1",
    "expires_at":"2030-01-01T00:00:00Z","refresh":{"refresh_token":"r1","client_id":"c","scope":"read",
    "token_endpoint":"https://auth/token","token_endpoint_auth":{"type":"client_secret_basic","client_secret":"s1"}}}"#;

fn message(e: ApiError) -> String {
    e.message
}

#[test]
fn a_new_access_token_without_an_expiry_keeps_the_stored_one() {
    // VA-5.
    let CredentialAuthInput::McpOauth { access_token, expires_at, .. } =
        rotate(auth(OAUTH), update(r#"{"type":"mcp_oauth","access_token":"t2"}"#)).unwrap()
    else {
        panic!("the type changed")
    };
    assert_eq!(access_token, SecretString::new("t2"));
    assert_eq!(expires_at.unwrap().as_datetime().to_rfc3339(), "2030-01-01T00:00:00+00:00");
}

#[test]
fn the_token_endpoint_client_id_and_server_url_are_not_fields_of_an_update() {
    // VA-4: live answers each as an unknown field.
    for body in [
        r#"{"type":"mcp_oauth","refresh":{"token_endpoint":"https://other/token"}}"#,
        r#"{"type":"mcp_oauth","refresh":{"client_id":"c2"}}"#,
        r#"{"type":"mcp_oauth","mcp_server_url":"https://b/mcp"}"#,
        r#"{"type":"environment_variable","secret_name":"OTHER"}"#,
    ] {
        let err = serde_json::from_str::<CredentialAuthUpdate>(body).unwrap_err().to_string();
        assert!(err.contains("unknown field"), "{body}: {err}");
    }
}

#[test]
fn a_client_secret_replaces_the_stored_one_under_the_stored_method() {
    let new = update(
        r#"{"type":"mcp_oauth","refresh":{"token_endpoint_auth":{"type":"client_secret_post","client_secret":"s2"}}}"#,
    );
    let CredentialAuthInput::McpOauth { refresh: Some(refresh), .. } = rotate(auth(OAUTH), new).unwrap()
    else {
        panic!("the refresh went")
    };
    assert_eq!(
        refresh.token_endpoint_auth,
        Some(TokenEndpointAuthInput::ClientSecretBasic { client_secret: SecretString::new("s2") })
    );
    assert_eq!((refresh.refresh_token, refresh.scope), (SecretString::new("r1"), Some("read".into())));
}

#[test]
fn a_secret_for_a_public_client_and_a_none_method_are_refused_with_live_s_messages() {
    let public = OAUTH.replace(r#""type":"client_secret_basic","client_secret":"s1""#, r#""type":"none""#);
    let secret = r#"{"type":"mcp_oauth","refresh":{"token_endpoint_auth":{"type":"client_secret_basic","client_secret":"x"}}}"#;
    assert_eq!(
        message(rotate(auth(&public), update(secret)).unwrap_err()),
        "auth.refresh.token_endpoint_auth: client_secret does not match this credential's stored auth method"
    );
    let none = r#"{"type":"mcp_oauth","refresh":{"token_endpoint_auth":{"type":"none"}}}"#;
    assert_eq!(
        message(rotate(auth(OAUTH), update(none)).unwrap_err()),
        "Failed to parse request: params.credential.auth.refresh.token_endpoint_auth.type: \"none\" is not a valid value"
    );
}

#[test]
fn a_refresh_update_needs_a_refresh_and_another_type_is_refused() {
    let plain = auth(r#"{"type":"mcp_oauth","mcp_server_url":"https://a/mcp","access_token":"t"}"#);
    assert_eq!(
        message(
            rotate(plain.clone(), update(r#"{"type":"mcp_oauth","refresh":{"refresh_token":"r"}}"#))
                .unwrap_err()
        ),
        "auth.refresh: this credential was created without refresh capability"
    );
    assert_eq!(
        message(rotate(plain, update(r#"{"type":"static_bearer","token":"x"}"#)).unwrap_err()),
        "auth.type: does not match this credential's stored type"
    );
}

#[test]
fn an_environment_variable_keeps_its_name_and_takes_new_hosts() {
    let env = auth(
        r#"{"type":"environment_variable","secret_name":"K","secret_value":"v1","networking":{"type":"unrestricted"}}"#,
    );
    let hosts = r#"{"type":"environment_variable","networking":{"type":"limited","allowed_hosts":["x.example.com"]}}"#;
    let CredentialAuthInput::EnvironmentVariable { secret_name, secret_value, networking, .. } =
        rotate(env, update(hosts)).unwrap()
    else {
        panic!("the type changed")
    };
    assert_eq!((secret_name.as_str(), secret_value), ("K", SecretString::new("v1")));
    assert!(matches!(networking, Some(crate::types::CredentialNetworking::Limited { .. })));
}
