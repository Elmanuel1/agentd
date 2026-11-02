//! End-to-end HTTP tests through the composed router: auth, beta gate,
//! error envelopes, and the cross-entity flows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use api::{build_router, AppState, Backends, EnvKeySource, Stores};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use http_body_util::BodyExt;
use secrets_blob::BlobSecretStore;
use shared::ports::fire::FireRegistry;
use shared::ports::{FakeRuntime, FakeScheduler};
use shared::Timestamp;
use store_postgres::PgBackend;
use tower::ServiceExt;

const KEY: &str = "test-key-workspace-a";
const KEY_B: &str = "test-key-workspace-b";

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1") && std::env::var("AGENTD_AWS_ENDPOINT").is_ok()
}

/// Build the composed router over live Postgres + object store, or `None` when
/// the integration env isn't configured (test skips). Unique tenants per call
/// isolate the shared DB.
async fn app() -> Option<Router> {
    if !enabled() {
        return None;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let backend = PgBackend::connect(&url).await.unwrap();
    let bucket = format!("agentdapi{}", Timestamp::now().as_datetime().timestamp_micros());
    let blobs: Arc<dyn BlobStore> = Arc::new(S3BlobStore::connect(&s3_config(bucket)).await.unwrap());
    let stamp = Timestamp::now().as_datetime().timestamp_micros();
    let org = format!("org_t{stamp}");
    let mut keys = EnvKeySource::default();
    keys.register(KEY, &org, &format!("wrkspc_a{stamp}"), "apikey_01a");
    keys.register(KEY_B, &org, &format!("wrkspc_b{stamp}"), "apikey_01b");
    let backends = Backends {
        stores: Stores::postgres(backend, blobs.clone(), Arc::new(BlobSecretStore::new(blobs))),
        keys: Arc::new(keys),
        scheduler: Arc::new(FakeScheduler::new()),
        fires: Arc::new(FireRegistry::new()),
        runtime: Arc::new(FakeRuntime::new()),
    };
    Some(build_router(AppState::build(backends).unwrap()))
}

/// A test bucket on the emulator at `AGENTD_AWS_ENDPOINT`, created on connect.
fn s3_config(bucket: String) -> S3Config {
    S3Config {
        endpoint: std::env::var("AGENTD_AWS_ENDPOINT").ok(),
        path_style: true,
        create_bucket: true,
        ..S3Config::new(bucket)
    }
}

/// Call with full header control.
async fn call_raw(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    api_key: Option<&str>,
    beta: bool,
) -> (StatusCode, serde_json::Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01");
    if let Some(k) = api_key {
        b = b.header("x-api-key", k);
    }
    if beta {
        b = b.header("anthropic-beta", beta_for(uri));
    }
    let req = b
        .body(match body {
            Some(s) => Body::from(s.to_owned()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

/// The beta the official SDK sends for `uri`.
fn beta_for(uri: &str) -> &'static str {
    if uri.starts_with("/v1/memory_stores") {
        "agent-memory-2026-07-22"
    } else {
        "managed-agents-2026-04-01"
    }
}

/// Standard authenticated + beta call.
async fn call(app: &Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, serde_json::Value) {
    call_raw(app, method, uri, body, Some(KEY), true).await
}

#[tokio::test]
async fn missing_api_key_is_401_envelope() {
    let Some(app) = app().await else {
        return;
    };
    let (st, v) = call_raw(&app, "GET", "/v1/agents", None, None, true).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "authentication_error");
    assert!(v["request_id"].as_str().unwrap().starts_with("req_01"));
}

#[tokio::test]
async fn unknown_api_key_is_401() {
    let Some(app) = app().await else {
        return;
    };
    let (st, v) = call_raw(&app, "GET", "/v1/agents", None, Some("sk-bogus"), true).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["type"], "authentication_error");
}

#[tokio::test]
async fn missing_beta_header_reproduces_live_unknown_field_error() {
    let Some(app) = app().await else {
        return;
    };
    // Live probe G: POST /v1/agents without beta →
    // 400 "Failed to parse request body: unknown field \"model\""
    let (st, v) = call_raw(
        &app,
        "POST",
        "/v1/agents",
        Some(r#"{"name":"x","model":"claude-sonnet-4-6"}"#),
        Some(KEY),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "Failed to parse request body: unknown field \"model\"");
    // GETs pass without the beta header (list still works).
    let (st, _) = call_raw(&app, "GET", "/v1/agents", None, Some(KEY), false).await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
async fn unknown_route_gets_wire_404_envelope() {
    let Some(app) = app().await else {
        return;
    };
    let (st, v) = call(&app, "GET", "/v1/nonexistent", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["type"], "not_found_error");
}

#[tokio::test]
async fn full_stack_agent_env_session_flow() {
    let Some(app) = app().await else {
        return;
    };
    // 1. environment
    let (st, env) = call(
        &app,
        "POST",
        "/v1/environments",
        Some(r#"{"name":"e2e-env","config":{"type":"cloud","networking":{"type":"unrestricted"}}}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(env["scope"], "organization");
    // 2. agent
    let (st, agent) = call(
        &app,
        "POST",
        "/v1/agents",
        Some(r#"{"name":"e2e-agent","model":"claude-sonnet-4-6","system":"probe","tools":[{"type":"agent_toolset_20260401"}]}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // 3. session referencing both
    let (st, session) = call(
        &app,
        "POST",
        "/v1/sessions",
        Some(&format!(
            r#"{{"agent":"{}","environment_id":"{}","title":"e2e"}}"#,
            agent["id"].as_str().unwrap(),
            env["id"].as_str().unwrap()
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(session["status"], "idle");
    assert_eq!(session["agent"]["id"], agent["id"]);
    assert!(session["id"].as_str().unwrap().starts_with("sesn_01"));
    // 4. send an event, list it back
    let sid = session["id"].as_str().unwrap();
    let (st, sent) = call(
        &app,
        "POST",
        &format!("/v1/sessions/{sid}/events"),
        Some(r#"{"events":[{"type":"user.message","content":[{"type":"text","text":"hello"}]}]}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sent["data"][0]["type"], "user.message");
    // Live: a queued event omits processed_at entirely (added once processed).
    assert!(sent["data"][0].get("processed_at").is_none());
    let (st, listed) = call(&app, "GET", &format!("/v1/sessions/{sid}/events"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn workspace_isolation_through_http() {
    let Some(app) = app().await else {
        return;
    };
    let (_, agent) =
        call(&app, "POST", "/v1/agents", Some(r#"{"name":"private","model":"claude-sonnet-4-6"}"#)).await;
    let id = agent["id"].as_str().unwrap();
    // Key B (same org, different workspace) cannot see the agent…
    let (st, _) = call_raw(&app, "GET", &format!("/v1/agents/{id}"), None, Some(KEY_B), true).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // …but CAN see the org-scoped environment.
    let (_, env) =
        call(&app, "POST", "/v1/environments", Some(r#"{"name":"shared","config":{"type":"self_hosted"}}"#))
            .await;
    let eid = env["id"].as_str().unwrap();
    let (st, got) = call_raw(&app, "GET", &format!("/v1/environments/{eid}"), None, Some(KEY_B), true).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], env["id"]);
}

#[tokio::test]
async fn deployment_create_and_internal_fire_through_http() {
    let Some(app) = app().await else {
        return;
    };
    let (_, env) =
        call(&app, "POST", "/v1/environments", Some(r#"{"name":"d-env","config":{"type":"self_hosted"}}"#))
            .await;
    let (_, agent) =
        call(&app, "POST", "/v1/agents", Some(r#"{"name":"d-agent","model":"claude-sonnet-4-6"}"#)).await;
    let (st, depl) = call(
        &app,
        "POST",
        "/v1/deployments",
        Some(&format!(
            r#"{{"name":"weekly","agent":"{}","environment_id":"{}",
                "initial_events":[{{"type":"user.message","content":[{{"type":"text","text":"run"}}]}}],
                "schedule":{{"type":"cron","expression":"0 20 * * 5","timezone":"America/New_York"}}}}"#,
            agent["id"].as_str().unwrap(),
            env["id"].as_str().unwrap()
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(depl["schedule"]["upcoming_runs_at"].as_array().unwrap().len(), 5);
    let did = depl["id"].as_str().unwrap();

    // Scheduler knocks on /internal/fire → session created + run recorded.
    let (st, fired) = call(
        &app,
        "POST",
        "/internal/fire",
        Some(&format!(r#"{{"deployment_id":"{did}","scheduled_at":"2026-07-10T00:00:00.000000Z"}}"#)),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(fired["fired"], true);
    assert!(fired["run"]["session_id"].as_str().unwrap().starts_with("sesn_01"));

    // The run is listable and the session got the kickoff event.
    let (st, runs) = call(&app, "GET", &format!("/v1/deployment_runs?deployment_id={did}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(runs["data"].as_array().unwrap().len(), 1);
    let sid = fired["run"]["session_id"].as_str().unwrap();
    let (_, events) = call(&app, "GET", &format!("/v1/sessions/{sid}/events"), None).await;
    assert_eq!(events["data"][0]["type"], "user.message");

    // Idempotency: same scheduled_at again → not fired.
    let (_, again) = call(
        &app,
        "POST",
        "/internal/fire",
        Some(&format!(r#"{{"deployment_id":"{did}","scheduled_at":"2026-07-10T00:00:00.000000Z"}}"#)),
    )
    .await;
    assert_eq!(again["fired"], false);
}

#[tokio::test]
async fn list_pagination_matches_live_cursor_shape() {
    let Some(app) = app().await else {
        return;
    };
    // Create 22 agents so a default page (20) leaves a next_page cursor.
    for i in 0..22 {
        call(
            &app,
            "POST",
            "/v1/agents",
            Some(&format!(r#"{{"name":"pg-{i}","model":"claude-sonnet-4-6"}}"#)),
        )
        .await;
    }
    // Default limit = 20, next_page cursor present (live shape).
    let (st, p1) = call(&app, "GET", "/v1/agents?limit=20", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(p1["data"].as_array().unwrap().len(), 20);
    let cursor = p1["next_page"].as_str().expect("more results → next_page cursor");
    assert!(cursor.starts_with("page_"));
    // Fetch the next page via ?page=<cursor> → the remaining 2, no next_page.
    let (_, p2) = call(&app, "GET", &format!("/v1/agents?limit=20&page={cursor}"), None).await;
    assert_eq!(p2["data"].as_array().unwrap().len(), 2);
    assert!(p2.get("next_page").is_none(), "last page omits next_page");
    // limit out of range → the exact live 400 message.
    let (st, err) = call(&app, "GET", "/v1/agents?limit=0", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        err["error"]["message"],
        "limit: value must be greater than or equal to 1 and less than or equal to 100"
    );
    // Unknown query param → live 400 "unknown field" (agentd allowlists limit/page).
    let (st, err) = call(&app, "GET", "/v1/agents?bogusparam=1", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"]["message"], "Failed to parse request body: unknown field \"bogusparam\"");
}

#[tokio::test]
async fn memory_patch_405_through_the_composed_router() {
    let Some(app) = app().await else {
        return;
    };
    let (_, store) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
    let sid = store["id"].as_str().unwrap();
    let (_, mem) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/a.md","content":"one"}"#),
    )
    .await;
    let mid = mem["id"].as_str().unwrap();
    let (st, _) =
        call(&app, "PATCH", &format!("/v1/memory_stores/{sid}/memories/{mid}"), Some(r#"{"content":"two"}"#))
            .await;
    assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn exact_live_error_strings_survive_the_full_stack() {
    let Some(app) = app().await else {
        return;
    };
    let (st, v) = call(&app, "POST", "/v1/agents", Some(r#"{"name":"x"}"#)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "model: Field required");
    let (st, v) = call(&app, "POST", "/v1/agents", Some(r#"{"name":"x","model":"gpt-4"}"#)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "`model.id`: model \"gpt-4\": model is not supported");
    let (st, v) = call(&app, "GET", "/v1/agents/agent_00bogus", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "Invalid agent ID.");
    // Unknown BODY field → live 400 "unknown field" (deny_unknown_fields + ApiJson).
    let (st, v) =
        call(&app, "POST", "/v1/agents", Some(r#"{"name":"x","model":"claude-sonnet-4-6","bogus":1}"#)).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "Failed to parse request body: unknown field \"bogus\"");
}
