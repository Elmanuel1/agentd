//! HTTP route coverage for /v1/vaults + credentials.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_core::{AuthContext, OrgId, WorkspaceId};
use agentd_vaults::{routes, InMemoryVaultsStore, VaultsService};
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> Router {
    let svc = Arc::new(VaultsService::new(Arc::new(InMemoryVaultsStore::new())));
    let ctx = AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    };
    routes::router()
        .layer(Extension(svc))
        .layer(Extension(ctx))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(s) => Body::from(s.to_owned()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn vault_crud_over_http() {
    let app = app();
    let (st, vault) = call(&app, "POST", "/v1/vaults", Some(r#"{"display_name":"Alice"}"#)).await;
    assert_eq!(st, StatusCode::OK);
    let id = vault["id"].as_str().unwrap();

    let (st, listed) = call(&app, "GET", "/v1/vaults", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    let (st, got) = call(&app, "GET", &format!("/v1/vaults/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got, vault);

    let (st, updated) = call(
        &app,
        "POST",
        &format!("/v1/vaults/{id}"),
        Some(r#"{"display_name":"Alice Prod"}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(updated["display_name"], "Alice Prod");

    let (st, archived) = call(&app, "POST", &format!("/v1/vaults/{id}/archive"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(archived["archived_at"].is_string());

    let (st, _) = call(&app, "DELETE", &format!("/v1/vaults/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&app, "GET", &format!("/v1/vaults/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn credential_crud_over_http_never_leaks_secret() {
    let app = app();
    let (_, vault) = call(&app, "POST", "/v1/vaults", Some(r#"{"display_name":"V"}"#)).await;
    let vid = vault["id"].as_str().unwrap();

    let (st, cred) = call(
        &app,
        "POST",
        &format!("/v1/vaults/{vid}/credentials"),
        Some(
            r#"{"display_name":"Twilio","auth":{"type":"environment_variable","secret_name":"TWILIO_API_KEY","secret_value":"sk-hyper-secret","networking":{"type":"limited","allowed_hosts":["api.twilio.com"]}}}"#,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!cred.to_string().contains("sk-hyper-secret"));
    assert_eq!(cred["auth"]["secret_name"], "TWILIO_API_KEY");
    let cid = cred["id"].as_str().unwrap();

    // list + get: still no secret anywhere.
    let (st, listed) = call(&app, "GET", &format!("/v1/vaults/{vid}/credentials"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!listed.to_string().contains("sk-hyper-secret"));
    let (st, got) = call(
        &app,
        "GET",
        &format!("/v1/vaults/{vid}/credentials/{cid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], cred["id"]);

    // rotate
    let (st, rotated) = call(
        &app,
        "POST",
        &format!("/v1/vaults/{vid}/credentials/{cid}"),
        Some(
            r#"{"auth":{"type":"environment_variable","secret_name":"TWILIO_API_KEY","secret_value":"sk-rotated"}}"#,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!rotated.to_string().contains("sk-rotated"));

    // archive purges + delete
    let (st, archived) = call(
        &app,
        "POST",
        &format!("/v1/vaults/{vid}/credentials/{cid}/archive"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(archived["archived_at"].is_string());
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/v1/vaults/{vid}/credentials/{cid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/v1/vaults/{vid}/credentials/{cid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn vault_error_paths_over_http() {
    let app = app();
    let (st, v) = call(&app, "POST", "/v1/vaults", Some("{}")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "display_name: Field required");
    let (st, v) = call(&app, "GET", "/v1/vaults/vlt_00bogus", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "Invalid vault ID.");
    // credential into a missing vault
    let ghost = agentd_vaults::VaultId::generate();
    let (st, _) = call(
        &app,
        "POST",
        &format!("/v1/vaults/{ghost}/credentials"),
        Some(r#"{"display_name":"x","auth":{"type":"static_bearer","mcp_server_url":"https://a/mcp","token":"t"}}"#),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
