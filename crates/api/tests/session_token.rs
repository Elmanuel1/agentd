//! A session's own work token on the public API, as the harness in its
//! sandbox uses it through the official SDK: it reports the agent's events on
//! `POST …/events`, saves and replaces the session's outputs through the
//! files API, and downloads the files the session mounts. Every other caller
//! keeps the client behaviour.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use api::config::SandboxConfig;
use api::{build_router, AppState, Backends, Stores};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use http_body_util::BodyExt;
use shared::ports::fire::FireRegistry;
use shared::ports::{FakeRuntime, FakeScheduler};
use shared::Timestamp;
use store_postgres::PgBackend;
use test_support::{MemorySecretStore, StaticAdminKeys, StaticKeySource};
use tower::ServiceExt;

const KEY: &str = "test-key-session-token";
const BOUNDARY: &str = "agentdboundary";

struct Fixture {
    app: Router,
    runtime: Arc<FakeRuntime>,
}

async fn fixture() -> Option<Fixture> {
    let enabled = std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
        && std::env::var("AGENTD_AWS_ENDPOINT").is_ok();
    if !enabled {
        return None;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let backend = PgBackend::connect(&url).await.unwrap();
    let stamp = Timestamp::now().as_datetime().timestamp_micros();
    let config = S3Config {
        endpoint: std::env::var("AGENTD_AWS_ENDPOINT").ok(),
        path_style: true,
        create_bucket: true,
        ..S3Config::new(format!("agentdtoken{stamp}"))
    };
    let blobs: Arc<dyn BlobStore> = Arc::new(S3BlobStore::connect(&config).await.unwrap());
    let mut keys = StaticKeySource::default();
    keys.register(KEY, &format!("org_t{stamp}"), &format!("wrkspc_t{stamp}"), "apikey_01t");
    let runtime = Arc::new(FakeRuntime::new());
    let backends = Backends {
        stores: Stores::postgres(backend, blobs, Arc::new(MemorySecretStore::default())),
        keys: Arc::new(keys),
        admin_keys: Arc::new(StaticAdminKeys::default()),
        scheduler: Arc::new(FakeScheduler::new()),
        fires: Arc::new(FireRegistry::new()),
        runtime: Some(runtime.clone()),
        sandbox: Some(SandboxConfig { api_url: "http://agentd:8080".into(), ..SandboxConfig::default() }),
        extra_models: Vec::new(),
        metrics: Arc::new(api::telemetry::Metrics::global()),
    };
    Some(Fixture { app: build_router(AppState::build(backends).unwrap()), runtime })
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    auth: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>) {
    let beta =
        if uri.starts_with("/v1/files") { "files-api-2025-04-14" } else { "managed-agents-2026-04-01" };
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {auth}"))
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", beta)
        .header("content-type", content_type)
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
}

async fn json(
    app: &Router,
    method: &str,
    uri: &str,
    auth: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let (status, bytes) = call(app, method, uri, auth, "application/json", body.as_bytes().to_vec()).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

async fn upload(app: &Router, auth: &str, name: &str, data: &str) -> (StatusCode, serde_json::Value) {
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n{data}\r\n--{BOUNDARY}--\r\n"
    );
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    let (status, bytes) = call(app, "POST", "/v1/files", auth, &content_type, body.into_bytes()).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

/// A session with `file` mounted, and its sandbox's sessions token.
async fn session(f: &Fixture, file: &str) -> (String, String) {
    let env_body = r#"{"name":"tok","config":{"type":"cloud","networking":{"type":"unrestricted"}}}"#;
    let (_, env) = json(&f.app, "POST", "/v1/environments", KEY, env_body).await;
    let (_, agent) =
        json(&f.app, "POST", "/v1/agents", KEY, r#"{"name":"tok","model":"claude-sonnet-4-6"}"#).await;
    let body = format!(
        r#"{{"agent":"{}","environment_id":"{}","resources":[{{"type":"file","file_id":"{file}"}}]}}"#,
        agent["id"].as_str().unwrap(),
        env["id"].as_str().unwrap(),
    );
    let (status, session) = json(&f.app, "POST", "/v1/sessions", KEY, &body).await;
    assert_eq!(status, StatusCode::OK, "{session}");
    let id = session["id"].as_str().unwrap().to_owned();
    let job = f.runtime.launched().into_iter().find(|j| j.target_id == id).unwrap();
    assert!(job.env.keys().all(|k| !k.contains("ENVIRONMENT_KEY")), "no environment key enters a sandbox");
    assert_eq!(job.env["ANTHROPIC_SESSION_ID"], id);
    let secret = URL_SAFE_NO_PAD.decode(job.env["ANTHROPIC_WORK_SECRET"].trim_end_matches('=')).unwrap();
    let secret: serde_json::Value = serde_json::from_slice(&secret).unwrap();
    (id, secret["sessions_token"].as_str().unwrap().to_owned())
}

#[tokio::test]
async fn the_session_token_reports_the_agent_s_side_and_stamps_what_the_agent_took() {
    let Some(f) = fixture().await else { return };
    let (_, input) = upload(&f.app, KEY, "in.txt", "brief").await;
    let (sid, token) = session(&f, input["id"].as_str().unwrap()).await;
    let events = format!("/v1/sessions/{sid}/events");
    let message = r#"{"events":[{"type":"user.message","content":[{"type":"text","text":"go"}]}]}"#;
    let (status, _) = json(&f.app, "POST", &events, KEY, message).await;
    assert_eq!(status, StatusCode::OK);
    // A batch holding a client event is refused before anything is stamped.
    let (status, _) = json(&f.app, "POST", &events, &token, message).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (_, listed) = json(&f.app, "GET", &events, KEY, "").await;
    assert!(listed["data"][0].get("processed_at").is_none(), "{listed}");
    let agent_side = r#"{"events":[{"type":"session.status_running"},
        {"type":"agent.message","content":[{"type":"text","text":"hi"}],"session_thread_id":null}]}"#;
    let (status, recorded) = json(&f.app, "POST", &events, &token, agent_side).await;
    assert_eq!(status, StatusCode::OK, "{recorded}");
    assert_eq!(recorded["data"][1]["type"], "agent.message");
    let (_, listed) = json(&f.app, "GET", &events, KEY, "").await;
    let first = &listed["data"][0];
    assert!(first["type"] == "user.message" && first["processed_at"].is_string(), "{listed}");
    let (_, s) = json(&f.app, "GET", &format!("/v1/sessions/{sid}"), KEY, "").await;
    assert_eq!(s["status"], "running");
    // A key still sends client events only.
    let (status, _) = json(&f.app, "POST", &events, KEY, agent_side).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (other, _) = session(&f, input["id"].as_str().unwrap()).await;
    let (status, _) = json(&f.app, "POST", &format!("/v1/sessions/{other}/events"), &token, agent_side).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_session_token_saves_replaces_and_reads_the_session_s_files() {
    let Some(f) = fixture().await else { return };
    let (_, input) = upload(&f.app, KEY, "in.txt", "brief").await;
    let input = input["id"].as_str().unwrap().to_owned();
    let (_, stray) = upload(&f.app, KEY, "stray.txt", "x").await;
    let stray = stray["id"].as_str().unwrap().to_owned();
    let (sid, token) = session(&f, &input).await;
    // The mounted upload is the session's to read, not a client's.
    let content = format!("/v1/files/{input}/content");
    let (status, bytes) = call(&f.app, "GET", &content, &token, "application/json", Vec::new()).await;
    assert!(status == StatusCode::OK && bytes == b"brief");
    let (status, _) = call(&f.app, "GET", &content, KEY, "application/json", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let stray_content = format!("/v1/files/{stray}/content");
    let (status, _) = call(&f.app, "GET", &stray_content, &token, "application/json", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A file an outcome of the session grades against is the session's to read too.
    let outcome = format!(
        r#"{{"events":[{{"type":"user.define_outcome","description":"d","rubric":{{"type":"file","file_id":"{stray}"}}}}]}}"#
    );
    let (status, _) = json(&f.app, "POST", &format!("/v1/sessions/{sid}/events"), KEY, &outcome).await;
    assert_eq!(status, StatusCode::OK);
    let (status, bytes) = call(&f.app, "GET", &stray_content, &token, "application/json", Vec::new()).await;
    assert!(status == StatusCode::OK && bytes == b"x");
    let (status, _) = call(&f.app, "GET", &stray_content, KEY, "application/json", Vec::new()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // An output is the session's, downloadable, and replaced by deleting its earlier version.
    let (status, first) = upload(&f.app, &token, "out.json", "{}").await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(first["downloadable"] == true && first["mime_type"] == "application/json");
    let (_, second) = upload(&f.app, &token, "out.json", "{\"a\":1}").await;
    let first = first["id"].as_str().unwrap();
    let (status, _) = json(&f.app, "DELETE", &format!("/v1/files/{first}"), &token, "").await;
    assert_eq!(status, StatusCode::OK);
    let (_, listed) = json(&f.app, "GET", &format!("/v1/files?scope_id={sid}"), KEY, "").await;
    let ids: Vec<_> = listed["data"].as_array().unwrap().iter().map(|f| f["id"].clone()).collect();
    assert_eq!(ids, vec![second["id"].clone()]);
    let (status, _) = json(&f.app, "DELETE", &format!("/v1/files/{stray}"), &token, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json(&f.app, "GET", "/v1/files", &token, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
