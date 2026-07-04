//! HTTP route coverage for /v1/sessions incl. the SSE stream endpoint and
//! the service paths not exercised elsewhere.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_agents::AgentSnapshot;
use agentd_core::{ApiError, AuthContext, OrgId, WorkspaceId};
use agentd_sessions::routes;
use agentd_sessions::service::{SessionDeps, SessionsService};
use agentd_sessions::store::InMemorySessionsStore;
use async_trait::async_trait;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

struct FakeDeps;

#[async_trait]
impl SessionDeps for FakeDeps {
    async fn resolve_agent(
        &self,
        _ctx: &AuthContext,
        _id: &str,
        _version: Option<u64>,
    ) -> Result<AgentSnapshot, ApiError> {
        let agent: agentd_agents::Agent =
            serde_json::from_str(include_str!("../../../fixtures/agent.create.json")).unwrap();
        Ok(agent.snapshot())
    }
    async fn check_environment(&self, _ctx: &AuthContext, _env: &str) -> Result<(), ApiError> {
        Ok(())
    }
}

fn app() -> Router {
    let svc = Arc::new(SessionsService::new(
        Arc::new(InMemorySessionsStore::new()),
        Arc::new(FakeDeps),
    ));
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

async fn create_session(app: &Router) -> serde_json::Value {
    let (st, s) = call(
        app,
        "POST",
        "/v1/sessions",
        Some(r#"{"agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm","environment_id":"env_01ok","title":"http","metadata":{"k":"v"}}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    s
}

#[tokio::test]
async fn session_crud_over_http() {
    let app = app();
    let session = create_session(&app).await;
    let id = session["id"].as_str().unwrap();

    let (st, listed) = call(&app, "GET", "/v1/sessions", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    let (st, got) = call(&app, "GET", &format!("/v1/sessions/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got, session);

    // update title + metadata
    let (st, updated) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{id}"),
        Some(r#"{"title":"renamed","metadata":{"a":"b"}}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(updated["title"], "renamed");
    assert_eq!(updated["metadata"]["a"], "b");

    // archive then writes conflict
    let (st, archived) = call(&app, "POST", &format!("/v1/sessions/{id}/archive"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(archived["archived_at"].is_string());
    let (st, err) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{id}"),
        Some(r#"{"title":"nope"}"#),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(err["error"]["type"], "invalid_request_error");

    // delete
    let (st, _) = call(&app, "DELETE", &format!("/v1/sessions/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&app, "GET", &format!("/v1/sessions/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn events_and_outcome_validation_over_http() {
    let app = app();
    let session = create_session(&app).await;
    let id = session["id"].as_str().unwrap();

    let (st, sent) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{id}/events"),
        Some(r#"{"events":[{"type":"user.message","content":[{"type":"text","text":"hi"}]},{"type":"user.interrupt"}]}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sent["data"].as_array().unwrap().len(), 2);

    let (st, listed) = call(&app, "GET", &format!("/v1/sessions/{id}/events"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 2);

    // define_outcome over HTTP with the exact live index-path error.
    let (st, err) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{id}/events"),
        Some(r#"{"events":[{"type":"user.define_outcome","description":"x","rubric":{"type":"text","content":"r"},"max_iterations":21}]}"#),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        err["error"]["message"],
        "events.0.user_define_outcome.max_iterations: value must be greater than or equal to 1 and less than or equal to 20"
    );
}

#[tokio::test]
async fn sse_stream_endpoint_responds_with_event_stream() {
    let app = app();
    let session = create_session(&app).await;
    let id = session["id"].as_str().unwrap();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/sessions/{id}/events/stream"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(ct.starts_with("text/event-stream"), "got {ct}");
    // Streaming for a missing session errors with the envelope instead.
    let ghost = agentd_sessions::types::SessionId::generate();
    let req = Request::builder()
        .method("GET")
        .uri(format!("/v1/sessions/{ghost}/events/stream"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_error_paths_over_http() {
    let app = app();
    let (st, v) = call(&app, "POST", "/v1/sessions", Some(r#"{"environment_id":"e"}"#)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "agent: Field required");
    let (st, v) = call(&app, "GET", "/v1/sessions/sesn_00bogus", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "Invalid session ID.");
}

#[tokio::test]
async fn session_resources_accepted_on_create() {
    let app = app();
    let (st, s) = call(
        &app,
        "POST",
        "/v1/sessions",
        Some(
            r#"{"agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm","environment_id":"env_01ok",
                "resources":[
                  {"type":"file","file_id":"file_01AbCdEfGhIjKlMnOpQrStUv","mount_path":"/workspace/data.csv"},
                  {"type":"github_repository","url":"https://github.com/o/r","authorization_token":"ghp_secret","checkout":{"type":"branch","name":"main"}},
                  {"type":"memory_store","memory_store_id":"memstore_01AbCdEfGhIjKlMnOpQrStUv","access":"read_only","instructions":"check first"}
                ],
                "vault_ids":["vlt_01AbCdEfGhIjKlMnOpQrStUv"]}"#,
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let resources = s["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 3);
    // Every resource got a sesrsc_ id; the repo token is NOT echoed.
    for r in resources {
        assert!(r["id"].as_str().unwrap().starts_with("sesrsc_01"));
    }
    assert!(!s.to_string().contains("ghp_secret"));
    assert_eq!(s["vault_ids"][0], "vlt_01AbCdEfGhIjKlMnOpQrStUv");
}
