//! Fixture conformance: replay the exact requests that produced the live
//! captures against agentd and assert the normalized responses are
//! structurally identical.
//!
//! Every fixture in `fixtures/` was captured from api.anthropic.com with
//! `anthropic-beta: managed-agents-2026-04-01` on 2026-07-03.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use test_support::{MemorySecretStore, StaticKeySource};

use api::{build_router, AppState, Backends, Stores};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use conformance::diff;
use http_body_util::BodyExt;
use shared::ports::fire::FireRegistry;
use shared::ports::{FakeRuntime, FakeScheduler};
use shared::Timestamp;
use store_postgres::PgBackend;
use tower::ServiceExt;

const KEY: &str = "conformance-key";

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1") && std::env::var("AGENTD_AWS_ENDPOINT").is_ok()
}

/// Compose the router over live Postgres + object store with a fresh tenant
/// (so empty-list conformance holds), or `None` when the env isn't configured.
async fn app() -> Option<Router> {
    if !enabled() {
        return None;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let backend = PgBackend::connect(&url).await.unwrap();
    let stamp = unique_stamp();
    let blobs: Arc<dyn BlobStore> = Arc::new(
        S3BlobStore::connect(&S3Config {
            endpoint: std::env::var("AGENTD_AWS_ENDPOINT").ok(),
            path_style: true,
            create_bucket: true,
            ..S3Config::new(format!("agentdconf{stamp}"))
        })
        .await
        .unwrap(),
    );
    let mut keys = StaticKeySource::default();
    keys.register(KEY, &format!("org_c{stamp}"), &format!("wrkspc_c{stamp}"), "apikey_01conf");
    let backends = Backends {
        stores: Stores::postgres(backend, blobs.clone(), Arc::new(MemorySecretStore::default())),
        keys: Arc::new(keys),
        scheduler: Arc::new(FakeScheduler::new()),
        fires: Arc::new(FireRegistry::new()),
        runtime: Some(Arc::new(FakeRuntime::new())),
        sandbox: None,
        extra_models: Vec::new(),
        metrics: Arc::new(api::telemetry::Metrics::global()),
    };
    Some(build_router(AppState::build(backends).unwrap()))
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("x-api-key", KEY)
        .header("anthropic-version", "2023-06-01")
        .header(
            "anthropic-beta",
            if uri.starts_with("/v1/memory_stores") {
                "agent-memory-2026-07-22"
            } else {
                "managed-agents-2026-04-01"
            },
        )
        .body(match body {
            Some(s) => Body::from(s.to_owned()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

fn fixture(name: &str) -> serde_json::Value {
    let raw = match name {
        "environment.create" => include_str!("../../fixtures/environment.create.json"),
        "agent.create" => include_str!("../../fixtures/agent.create.json"),
        "memory_store.create" => include_str!("../../fixtures/memory_store.create.json"),
        "memory.create" => include_str!("../../fixtures/memory.create.json"),
        "session.create" => include_str!("../../fixtures/session.create.json"),
        other => panic!("unknown fixture {other}"),
    };
    let mut v: serde_json::Value = serde_json::from_str(raw).unwrap();
    // The July captures predate `execution_identity` and a session's
    // `budget`, which the current API serves (SDK 1.3.0 types both as required).
    let identity = serde_json::json!({"type": "service_account"});
    match name {
        "agent.create" => v["execution_identity"] = identity,
        "session.create" => {
            v["agent"]["execution_identity"] = identity;
            v["budget"] = serde_json::Value::Null;
        }
        _ => {}
    }
    v
}

fn assert_conformant(name: &str, expected: &serde_json::Value, actual: &serde_json::Value) {
    let mismatches = diff(expected, actual);
    assert!(mismatches.is_empty(), "{name} diverges from the live capture:\n  {}", mismatches.join("\n  "));
}

#[tokio::test]
async fn environment_create_matches_live_capture() {
    let Some(app) = app().await else {
        return;
    };
    // The exact request from the live probe.
    let (st, actual) = call(
        &app,
        "POST",
        "/v1/environments",
        Some(r#"{"name":"compat-probe-env","config":{"type":"cloud","networking":{"type":"unrestricted"}}}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_conformant("environment.create", &fixture("environment.create"), &actual);
}

#[tokio::test]
async fn agent_create_matches_live_capture() {
    let Some(app) = app().await else {
        return;
    };
    let (st, actual) = call(
        &app,
        "POST",
        "/v1/agents",
        Some(r#"{"name":"compat-probe-agent","model":"claude-sonnet-4-6","system":"probe","tools":[{"type":"agent_toolset_20260401"}]}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_conformant("agent.create", &fixture("agent.create"), &actual);
}

#[tokio::test]
async fn memory_store_create_matches_live_capture() {
    let Some(app) = app().await else {
        return;
    };
    let (st, actual) =
        call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"compat-probe-mem","description":"probe"}"#))
            .await;
    assert_eq!(st, StatusCode::OK);
    assert_conformant("memory_store.create", &fixture("memory_store.create"), &actual);
}

#[tokio::test]
async fn memory_create_matches_live_capture() {
    let Some(app) = app().await else {
        return;
    };
    let (_, store) =
        call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"compat-probe-mem","description":"probe"}"#))
            .await;
    let sid = store["id"].as_str().unwrap();
    let (st, actual) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/rules.md","content":"v1 content"}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // The live sha256 of "v1 content" must match EXACTLY (not normalized
    // away — same input → same hash on both servers).
    assert_eq!(actual["content_sha256"], "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55");
    assert_conformant("memory.create", &fixture("memory.create"), &actual);
}

#[tokio::test]
async fn session_create_matches_live_capture() {
    let Some(app) = app().await else {
        return;
    };
    let (_, env) = call(
        &app,
        "POST",
        "/v1/environments",
        Some(r#"{"name":"compat-probe-env","config":{"type":"cloud","networking":{"type":"unrestricted"}}}"#),
    )
    .await;
    let (_, agent) = call(
        &app,
        "POST",
        "/v1/agents",
        Some(r#"{"name":"compat-probe-agent","model":"claude-sonnet-4-6","system":"probe","tools":[{"type":"agent_toolset_20260401"}]}"#),
    )
    .await;
    let (st, actual) = call(
        &app,
        "POST",
        "/v1/sessions",
        Some(&format!(
            r#"{{"agent":"{}","environment_id":"{}","title":"probe"}}"#,
            agent["id"].as_str().unwrap(),
            env["id"].as_str().unwrap()
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_conformant("session.create", &fixture("session.create"), &actual);
}

#[tokio::test]
async fn error_envelopes_match_live_captures() {
    let Some(app) = app().await else {
        return;
    };
    // Each (request, expected error body) pair captured live.
    let cases: Vec<(&str, &str, Option<&str>, u16, serde_json::Value)> = vec![
        (
            "POST",
            "/v1/agents",
            Some(r#"{"name":"x"}"#),
            400,
            serde_json::json!({"type":"error","error":{"type":"invalid_request_error","message":"model: Field required"},"request_id":"req_011CcfvHvqyhVMVsfTNRo4SG"}),
        ),
        (
            "GET",
            "/v1/agents/agent_00bogus",
            None,
            400,
            serde_json::json!({"type":"error","error":{"type":"invalid_request_error","message":"Invalid agent ID."},"request_id":"req_011CcfvHwY92XABALiTyT5E1"}),
        ),
        (
            "POST",
            "/v1/agents",
            Some(r#"{"name":"x","model":"gpt-4"}"#),
            400,
            serde_json::json!({"type":"error","error":{"type":"invalid_request_error","message":"`model.id`: model \"gpt-4\": model is not supported"},"request_id":"req_011CcfvHxBqUbtrVL335TgLY"}),
        ),
    ];
    for (method, uri, body, status, expected) in cases {
        let (st, actual) = call(&app, method, uri, body).await;
        assert_eq!(st.as_u16(), status, "{method} {uri}");
        assert_conformant(&format!("{method} {uri}"), &expected, &actual);
    }
}

#[tokio::test]
async fn memory_conflict_409_matches_live_capture() {
    let Some(app) = app().await else {
        return;
    };
    let (_, store) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
    let sid = store["id"].as_str().unwrap();
    call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/a.md","content":"one"}"#),
    )
    .await;
    let (st, actual) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/a.md","content":"dup"}"#),
    )
    .await;
    assert_eq!(st.as_u16(), 409);
    // Live capture (ids normalized by the differ; backtick message has the
    // id inline so compare the surrounding shape + typed fields).
    assert_eq!(actual["error"]["type"], "memory_path_conflict_error");
    assert_eq!(actual["error"]["conflicting_path"], "/a.md");
    assert!(actual["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("path `/a.md` is already used by `mem_"));
    assert!(actual["error"]["message"].as_str().unwrap().ends_with("`; use update to modify it"));
}

#[tokio::test]
async fn empty_list_shape_matches_live() {
    let Some(app) = app().await else {
        return;
    };
    // Live: GET /v1/agents → {"data":[]}
    let (st, v) = call(&app, "GET", "/v1/agents", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, serde_json::json!({"data":[]}));
}

/// A stamp no other app in this test binary shares: tests run in parallel, and
/// two apps built in the same microsecond would share a workspace.
fn unique_stamp() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}x{n}", Timestamp::now().as_datetime().timestamp_micros())
}
