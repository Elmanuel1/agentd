//! Vaults through HTTP, as live answers them (probed 2026-11-15): a session
//! names only usable vaults (VA-2) and cannot change them (VA-1), and a
//! credential reads with live's shape. Gated by `AGENTD_PG_TESTS=1` and
//! `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use axum::http::StatusCode;
use common::{Auth, Rig, KEY};
use serde_json::{json, Value};

async fn vault(rig: &Rig) -> String {
    let (st, v) = rig.call("POST", "/v1/vaults", Auth::Key(KEY), Some(json!({"display_name": "v"}))).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

fn error(body: &Value) -> (&str, &str) {
    (body["error"]["type"].as_str().unwrap(), body["error"]["message"].as_str().unwrap())
}

#[tokio::test]
async fn a_session_names_only_usable_vaults_and_cannot_change_them() {
    let Some(rig) = Rig::new().await else { return };
    let env = rig.environment("self_hosted").await;
    let (open, archived) = (vault(&rig).await, vault(&rig).await);
    let (st, _) = rig.call("POST", &format!("/v1/vaults/{archived}/archive"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    let (_, agent) = rig
        .call("POST", "/v1/agents", Auth::Key(KEY), Some(json!({"name": "va", "model": "claude-sonnet-4-6"})))
        .await;
    let create = |vaults: Value| json!({"agent": agent["id"], "environment_id": env, "vault_ids": vaults});

    // VA-2.
    let (st, body) = rig.call("POST", "/v1/sessions", Auth::Key(KEY), Some(create(json!([archived])))).await;
    let is_archived = format!("vault {archived} is archived");
    assert_eq!(
        (st, error(&body)),
        (StatusCode::BAD_REQUEST, ("invalid_request_error", is_archived.as_str()))
    );
    let unknown = "vlt_01AAAAAAAAAAAAAAAAAAAAAA";
    let (st, body) = rig.call("POST", "/v1/sessions", Auth::Key(KEY), Some(create(json!([unknown])))).await;
    let not_found = format!("vault {unknown} not found");
    assert_eq!((st, error(&body)), (StatusCode::NOT_FOUND, ("not_found_error", not_found.as_str())));

    // VA-1.
    let session = rig.session(&env, json!({"vault_ids": [open]})).await;
    let (st, body) = rig
        .call("POST", &format!("/v1/sessions/{session}"), Auth::Key(KEY), Some(json!({"vault_ids": []})))
        .await;
    assert_eq!(
        (st, error(&body)),
        (StatusCode::BAD_REQUEST, ("invalid_request_error", "Updating vault_ids is not yet supported."))
    );
}

#[tokio::test]
async fn a_credential_reads_with_live_s_shape() {
    let Some(rig) = Rig::new().await else { return };
    let v = vault(&rig).await;
    let auth = json!({"type": "mcp_oauth", "mcp_server_url": "https://a.example.com/mcp", "access_token": "t",
        "expires_at": "2030-01-01T00:00:00Z", "refresh": {"refresh_token": "r", "client_id": "c",
        "token_endpoint": "https://auth.example.com/token",
        "token_endpoint_auth": {"type": "client_secret_post", "client_secret": "s"}}});
    let (st, c) = rig
        .call(
            "POST",
            &format!("/v1/vaults/{v}/credentials"),
            Auth::Key(KEY),
            Some(json!({"display_name": "o", "auth": auth})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{c}");
    assert!(c["id"].as_str().unwrap().starts_with("vcrd_"));
    assert_eq!(c["type"], "vault_credential");
    assert_eq!(
        c["auth"],
        json!({"type": "mcp_oauth", "mcp_server_url": "https://a.example.com/mcp", "expires_at": "2030-01-01T00:00:00Z",
            "refresh": {"client_id": "c", "token_endpoint": "https://auth.example.com/token",
                "token_endpoint_auth": {"type": "client_secret_post"}}})
    );
    let id = c["id"].as_str().unwrap();
    let rotate = json!({"auth": {"type": "mcp_oauth", "access_token": "t2"}});
    let (st, rotated) =
        rig.call("POST", &format!("/v1/vaults/{v}/credentials/{id}"), Auth::Key(KEY), Some(rotate)).await;
    assert_eq!(
        (st, &rotated["auth"]["expires_at"]),
        (StatusCode::OK, &json!("2030-01-01T00:00:00Z")),
        "VA-5"
    );
    let endpoint =
        json!({"auth": {"type": "mcp_oauth", "refresh": {"token_endpoint": "https://other/token"}}});
    let (st, body) =
        rig.call("POST", &format!("/v1/vaults/{v}/credentials/{id}"), Auth::Key(KEY), Some(endpoint)).await;
    assert_eq!(
        (st, error(&body).1),
        (StatusCode::BAD_REQUEST, "Failed to parse request body: unknown field \"token_endpoint\""),
        "VA-4"
    );
}
