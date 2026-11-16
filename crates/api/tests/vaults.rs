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

/// A server for `mcp_oauth_validate`: `/ok` answers `initialize`, `/locked`
/// refuses it, `/token` grants a token and `/badtoken` refuses the grant.
async fn validation_server() -> (String, shared::net::Guard, std::sync::Arc<parking_lot::Mutex<Vec<String>>>)
{
    use axum::routing::post;
    let forms = std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));
    let seen = forms.clone();
    let app = axum::Router::new()
        .route(
            "/ok",
            post(|| async {
                axum::Json(json!({"jsonrpc": "2.0", "id": 1, "result": {"protocolVersion": "2025-11-25"}}))
            }),
        )
        .route(
            "/locked",
            post(|| async { (StatusCode::UNAUTHORIZED, axum::Json(json!({"error": "unauthorized"}))) }),
        )
        .route(
            "/token",
            post(move |body: String| async move {
                seen.lock().push(body);
                axum::Json(json!({"access_token": "fresh", "token_type": "Bearer", "expires_in": 3600}))
            }),
        )
        .route(
            "/badtoken",
            post(|| async { (StatusCode::BAD_REQUEST, axum::Json(json!({"error": "invalid_grant"}))) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (format!("http://{addr}"), shared::net::Guard::allowing(vec![addr]), forms)
}

#[tokio::test]
async fn mcp_oauth_validate_answers_as_live_does() {
    // VA-6, B23; live's answers captured 2026-11-15.
    let Some(rig) = Rig::new().await else { return };
    let (base, guard, forms) = validation_server().await;
    let app = rig.replica(&rig.stores, common::Streaming { guard, ..common::Streaming::default() });
    let v = vault(&rig).await;
    let validate = |auth: Value| {
        let app = app.clone();
        let v = v.clone();
        async move {
            let path = format!("/v1/vaults/{v}/credentials");
            let body = json!({"display_name": "c", "auth": auth});
            let (st, cred) =
                common::send_to(&app, common::request("POST", &path, Auth::Key(KEY), Some(body))).await;
            assert_eq!(st, StatusCode::OK, "{cred}");
            let path =
                format!("/v1/vaults/{v}/credentials/{}/mcp_oauth_validate", cred["id"].as_str().unwrap());
            let (st, got) = common::send_to(&app, common::request("POST", &path, Auth::Key(KEY), None)).await;
            (st, got, cred["id"].as_str().unwrap().to_owned())
        }
    };
    let oauth = |url: String, refresh: Option<&str>| {
        let mut auth = json!({"type": "mcp_oauth", "mcp_server_url": url, "access_token": "old"});
        if let Some(endpoint) = refresh {
            auth["refresh"] = json!({"client_id": "c", "token_endpoint": format!("{base}/{endpoint}"),
                "token_endpoint_auth": {"type": "none"}, "refresh_token": "r", "scope": "s"});
        }
        auth
    };

    let (st, ok, _) = validate(oauth(format!("{base}/ok"), None)).await;
    assert_eq!(st, StatusCode::OK, "{ok}");
    assert_eq!(
        (ok["type"].as_str(), ok["status"].as_str()),
        (Some("vault_credential_validation"), Some("valid"))
    );
    assert_eq!(
        (&ok["mcp_probe"], &ok["refresh"], &ok["has_refresh_token"]),
        (&Value::Null, &Value::Null, &json!(false))
    );
    assert!(ok["validated_at"].is_string());

    let (_, locked, _) = validate(oauth(format!("{base}/locked"), None)).await;
    assert_eq!(locked["status"], "invalid");
    assert_eq!(locked["mcp_probe"]["method"], "initialize");
    assert_eq!(locked["mcp_probe"]["http_response"]["status_code"], 401);
    assert_eq!(locked["mcp_probe"]["http_response"]["body_truncated"], false);
    assert_eq!(locked["refresh"], json!({"status": "no_refresh_token", "http_response": null}));

    let (_, refreshed, id) = validate(oauth(format!("{base}/locked?a"), Some("token"))).await;
    assert_eq!(
        (refreshed["status"].as_str(), refreshed["has_refresh_token"].as_bool()),
        (Some("unknown"), Some(true))
    );
    assert_eq!(refreshed["refresh"], json!({"status": "succeeded", "http_response": null}));
    assert_eq!(
        forms.lock().as_slice(),
        ["grant_type=refresh_token&refresh_token=r&client_id=c"],
        "no scope, as live"
    );
    let (_, cred) = rig.call("GET", &format!("/v1/vaults/{v}/credentials/{id}"), Auth::Key(KEY), None).await;
    assert!(cred["auth"]["expires_at"].is_string(), "the new token's expiry is stored: {cred}");

    let (_, refused, _) = validate(oauth(format!("{base}/locked?b"), Some("badtoken"))).await;
    assert_eq!(refused["status"], "invalid");
    assert_eq!(
        (refused["refresh"]["status"].as_str(), refused["refresh"]["http_response"]["status_code"].as_u64()),
        (Some("failed"), Some(400))
    );

    let (_, gone, _) = validate(oauth("https://agentd-validate.invalid/mcp".into(), None)).await;
    assert_eq!(
        (gone["status"].as_str(), &gone["mcp_probe"]),
        (Some("unknown"), &json!({"method": "initialize", "http_response": null}))
    );

    let bearer = json!({"type": "static_bearer", "mcp_server_url": format!("{base}/bearer"), "token": "t"});
    let (st, body, _) = validate(bearer).await;
    assert_eq!(
        (st, error(&body)),
        (
            StatusCode::BAD_REQUEST,
            ("invalid_request_error", "Credentials of this auth.type cannot be validated.")
        )
    );
}

#[tokio::test]
async fn a_lost_refresh_claim_uses_the_winner_s_token_or_waits() {
    // MC-7, MC-20, MC-23: one token request for two refreshes of one read.
    use std::sync::Arc;
    use vaults::mcp::Refresh;
    use vaults::store::{StoredCredential, VaultCredentialStore};
    let Some(rig) = Rig::new().await else { return };
    let (base, guard, forms) = validation_server().await;
    let probe = Arc::new(engine::mcp::Probe::new(guard.clone(), std::time::Duration::from_secs(5)));
    let svc = vaults::VaultsService::new(rig.stores.vaults.clone(), rig.stores.vault_credentials.clone())
        .with_mcp(vaults::McpReach { probe, guard });
    let v = vault(&rig).await;
    let auth = json!({"type": "mcp_oauth", "mcp_server_url": format!("{base}/locked"), "access_token": "old",
        "expires_at": "2020-01-01T00:00:00Z", "refresh": {"client_id": "c", "token_endpoint": format!("{base}/token"),
        "token_endpoint_auth": {"type": "none"}, "refresh_token": "r"}});
    let path = format!("/v1/vaults/{v}/credentials");
    let (_, cred) =
        rig.call("POST", &path, Auth::Key(KEY), Some(json!({"display_name": "c", "auth": auth}))).await;
    let (ws, org) = (shared::WorkspaceId::new(rig.ws.clone()), shared::OrgId::new(rig.org.clone()));
    let (vid, cid) = (
        vaults::VaultId::parse(&v).unwrap(),
        vaults::types::CredentialId::parse(cred["id"].as_str().unwrap()).unwrap(),
    );
    let read = |store: Arc<dyn VaultCredentialStore>| {
        let (ws, vid, cid) = (ws.clone(), vid.clone(), cid.clone());
        async move { store.get_credential(&ws, &vid, &cid).await.unwrap() }
    };
    let first: StoredCredential = read(rig.stores.vault_credentials.clone()).await;

    // Another writer holds a claim and has not written yet: the stored token is still expired.
    let held = rig.stores.vault_credentials.claim_credential(
        &ws,
        &vid,
        &cid,
        first.wire.updated_at,
        shared::Timestamp::now(),
    );
    assert!(held.await.unwrap());
    assert!(matches!(svc.refresh_mcp_oauth(&ws, &org, first.clone(), false).await.unwrap(), Refresh::Busy));
    assert!(forms.lock().is_empty(), "the loser calls no token endpoint");

    // The winner refreshes; a second refresh of the same read uses its token.
    let now: StoredCredential = read(rig.stores.vault_credentials.clone()).await;
    let Refresh::Token(won, _) = svc.refresh_mcp_oauth(&ws, &org, now.clone(), false).await.unwrap() else {
        panic!()
    };
    let Refresh::Token(lost, _) = svc.refresh_mcp_oauth(&ws, &org, now, false).await.unwrap() else {
        panic!()
    };
    assert_eq!((won.bearer.expose(), lost.bearer.expose()), ("fresh", "fresh"));
    assert_eq!(forms.lock().len(), 1, "one token request");
}
