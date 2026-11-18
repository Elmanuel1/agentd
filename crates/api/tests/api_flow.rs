//! End-to-end HTTP tests through the composed router: auth, beta gate,
//! error envelopes, and the cross-entity flows.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use test_support::{MemorySecretStore, StaticAdminKeys, StaticKeySource};

use api::{AppState, Backends, Stores, build_router};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use http_body_util::BodyExt;
use shared::ports::fire::FireRegistry;

use shared::Timestamp;
use store_postgres::{PgBackend, PgFireClaims};
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
    let bucket = format!("agentdapi{}", unique_stamp());
    let blobs: Arc<dyn BlobStore> = Arc::new(S3BlobStore::connect(&s3_config(bucket)).await.unwrap());
    let stamp = unique_stamp();
    let org = format!("org_t{stamp}");
    let mut keys = StaticKeySource::default();
    keys.register(KEY, &org, &format!("wrkspc_a{stamp}"), "apikey_01a");
    keys.register(KEY_B, &org, &format!("wrkspc_b{stamp}"), "apikey_01b");
    let load = Arc::new(store_postgres::PgLoad::new(backend.diesel.clone()));
    let backends = Backends {
        fires: Arc::new(FireRegistry::new(Arc::new(PgFireClaims::new(backend.clone())))),
        fire_sources: None,
        stores: Stores::postgres(backend, blobs.clone(), Arc::new(MemorySecretStore::default())),
        keys: Arc::new(keys),
        admin_keys: Arc::new(StaticAdminKeys::default()),
        environment_keys: Arc::new(test_support::NoEnvironmentKeys),
        system: api::SystemKeys {
            work_tokens: shared::hash::KeyRing::single(vec![1; 32]),
            placeholders: shared::hash::KeyRing::single(vec![2; 32]),
            api_base_url: "http://agentd.test".into(),
        },
        extra_models: Vec::new(),
        metrics: Arc::new(api::telemetry::Metrics::global()),
        signals: Arc::new(sessions::signals::LocalSignals::default()),
        load,
        role: api::config::Role::All,
        guard: shared::net::Guard::default(),
        webhooks: std::sync::Arc::default(),
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
    if uri.starts_with("/v1/memory_stores") { "agent-memory-2026-07-22" } else { "managed-agents-2026-04-01" }
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

    // The same tick again finds the first fire's run and session: no second
    // run, session or kickoff.
    let (_, again) = call(
        &app,
        "POST",
        "/internal/fire",
        Some(&format!(r#"{{"deployment_id":"{did}","scheduled_at":"2026-07-10T00:00:00.000000Z"}}"#)),
    )
    .await;
    assert_eq!(again["run"]["id"], fired["run"]["id"]);
    assert_eq!(again["run"]["session_id"], fired["run"]["session_id"]);
    let (_, runs) = call(&app, "GET", &format!("/v1/deployment_runs?deployment_id={did}"), None).await;
    assert_eq!(runs["data"].as_array().unwrap().len(), 1);
    let (_, events) = call(&app, "GET", &format!("/v1/sessions/{sid}/events"), None).await;
    let kickoffs = events["data"].as_array().unwrap().iter().filter(|e| e["type"] == "user.message").count();
    assert_eq!(kickoffs, 1);
}

#[tokio::test]
async fn a_fire_whose_agent_reaches_a_blocked_mcp_host_pauses_the_deployment() {
    // MC-18, MC-19.
    let Some(app) = app().await else {
        return;
    };
    let env =
        |networking: &str| format!(r#"{{"name":"m","config":{{"type":"cloud","networking":{networking}}}}}"#);
    let (_, closed) = call(
        &app,
        "POST",
        "/v1/environments",
        Some(&env(r#"{"type":"limited","allowed_hosts":["other.com"]}"#)),
    )
    .await;
    let (_, open) =
        call(&app, "POST", "/v1/environments", Some(&env(r#"{"type":"limited","allow_mcp_servers":true}"#)))
            .await;
    let agent = r#"{"name":"m","model":"claude-sonnet-4-6",
        "mcp_servers":[{"type":"url","name":"docs","url":"https://mcp.example.com/mcp"}],
        "tools":[{"type":"mcp_toolset","mcp_server_name":"docs"}]}"#;
    let (st, agent) = call(&app, "POST", "/v1/agents", Some(agent)).await;
    assert_eq!(st, StatusCode::OK, "{agent}");
    let deploy = |env: &serde_json::Value| {
        format!(
            r#"{{"name":"m","agent":"{}","environment_id":"{}","initial_events":[{{"type":"user.message","content":[{{"type":"text","text":"go"}}]}}],"schedule":{{"type":"cron","expression":"0 20 * * 5","timezone":"UTC"}}}}"#,
            agent["id"].as_str().unwrap(),
            env["id"].as_str().unwrap()
        )
    };
    let fire =
        |id: &str| format!(r#"{{"deployment_id":"{id}","scheduled_at":"2026-07-10T00:00:00.000000Z"}}"#);
    let (_, blocked) = call(&app, "POST", "/v1/deployments", Some(&deploy(&closed))).await;
    let did = blocked["id"].as_str().unwrap_or_else(|| panic!("{blocked}"));
    let (st, fired) = call(&app, "POST", "/internal/fire", Some(&fire(did))).await;
    assert_eq!(st, StatusCode::OK, "{fired}");
    assert_eq!(fired["run"]["session_id"], serde_json::Value::Null, "no session is created");
    let error = serde_json::json!({"type": "mcp_egress_blocked_error",
        "message": "An MCP server host used by the deployment's agent is blocked by the environment's network policy"});
    assert_eq!(fired["run"]["error"], error);
    let (_, got) = call(&app, "GET", &format!("/v1/deployments/{did}"), None).await;
    assert_eq!(got["status"], "paused");
    assert_eq!(got["paused_reason"], serde_json::json!({"type": "error", "error": error}));

    let (_, allowed) = call(&app, "POST", "/v1/deployments", Some(&deploy(&open))).await;
    let (_, fired) = call(&app, "POST", "/internal/fire", Some(&fire(allowed["id"].as_str().unwrap()))).await;
    assert!(fired["run"]["session_id"].as_str().is_some_and(|s| s.starts_with("sesn_")), "{fired}");
    let direct = format!(
        r#"{{"agent":"{}","environment_id":"{}"}}"#,
        agent["id"].as_str().unwrap(),
        closed["id"].as_str().unwrap()
    );
    let (st, _) = call(&app, "POST", "/v1/sessions", Some(&direct)).await;
    assert_eq!(st, StatusCode::OK, "a session created directly is not checked at create");
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

#[tokio::test]
async fn a_session_queues_its_work_item_on_its_environment_s_queue() {
    let Some(app) = app().await else {
        return;
    };
    let env_body = r#"{"name":"sbx","config":{"type":"cloud","networking":{"type":"limited"}}}"#;
    let (_, env) = call(&app, "POST", "/v1/environments", Some(env_body)).await;
    let (_, agent) =
        call(&app, "POST", "/v1/agents", Some(r#"{"name":"sbx","model":"claude-sonnet-4-6"}"#)).await;
    let body = format!(
        r#"{{"agent":"{}","environment_id":"{}"}}"#,
        agent["id"].as_str().unwrap(),
        env["id"].as_str().unwrap()
    );
    let (st, session) = call(&app, "POST", "/v1/sessions", Some(&body)).await;
    assert_eq!(st, StatusCode::OK);
    let item =
        format!("/v1/environments/{}/work/{}", env["id"].as_str().unwrap(), session["id"].as_str().unwrap());
    let (st, work) = call(&app, "GET", &item, None).await;
    assert_eq!((st, work["state"].as_str()), (StatusCode::OK, Some("queued")), "{work}");
}

/// A stamp no other app in this test binary shares: tests run in parallel, and
/// two apps built in the same microsecond would share a workspace.
fn unique_stamp() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}x{n}", Timestamp::now().as_datetime().timestamp_micros())
}
