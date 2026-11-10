//! The HTTP edge against the live captures (`fixtures/live/2026-11-01/auth`,
//! `files/15-17`, `skills/13`): hermetic, over a stand-in module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use test_support::{StaticAdminKeys, StaticKeySource};

use api::edge;
use api::edge::reach::{Grants, SessionGrants};
use api::edge::scoped::WorkAccess;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use shared::beta::{BetaGate, BetaSet};
use shared::module::BodyLimit;
use shared::ports::keys::{EnvironmentGrant, EnvironmentKeySource, KeyError};
use shared::{
    ApiError, ApiJson, AuthContext, EnvironmentCaller, OrgId, Pagination, RouteError, Timestamp, WorkspaceId,
    Writer,
};
use std::time::Duration;
use tower::ServiceExt;
use work::store::{Counts, Reach, Scope, StoreError};
use work::{Work, WorkItem, WorkReads, WorkState};

const KEY: &str = "sk-edge";
const ADMIN_KEY: &str = "sk-edge-admin";
const WORK_KEY: &[u8] = b"edge-work";
/// A self-hosted environment's key, for `env_e`.
const ENV_KEY: &str = "sk-ant-oat01-edge-environment-key";
/// agentd's runtime key.
const RUNTIME_KEY: &str = "sk-ant-oat01-edge-runtime-key";
/// Live's 403 for an environment key off its routes.
const SCOPE: &str = "OAuth token does not meet scope requirement any_of(org:sessions:claude_code, user:developer, user:sessions:claude_code, workspace:developer)";

struct Keys;

#[async_trait::async_trait]
impl EnvironmentKeySource for Keys {
    async fn resolve(&self, key: &str) -> Result<Option<EnvironmentGrant>, KeyError> {
        let ctx = AuthContext {
            org: OrgId::new("org_e"),
            workspace: WorkspaceId::new("wrkspc_e"),
            api_key_id: "envkey_e".into(),
        };
        Ok(match key {
            ENV_KEY => Some(EnvironmentGrant::Environment(ctx, EnvironmentCaller { queue: "env_e".into() })),
            RUNTIME_KEY => Some(EnvironmentGrant::Runtime),
            _ => None,
        })
    }
}

/// `sesn_1`'s item is active; `sesn_stopped`'s is stopped; `sesn_rt` is on
/// the runtime queue in `env_rt`.
struct Items;

fn item(session: &str, queue: &str, state: WorkState) -> WorkItem {
    let work: Work = serde_json::from_value(serde_json::json!({
        "id": session, "acknowledged_at": null, "actor": null, "created_at": "2026-11-09T00:00:00Z",
        "data": {"id": session, "type": "session"}, "environment_id": "env_rt", "latest_heartbeat_at": null,
        "metadata": {}, "secret": null, "started_at": null, "state": state, "stop_requested_at": null,
        "stopped_at": null, "type": "work"
    }))
    .unwrap();
    WorkItem {
        workspace: WorkspaceId::new("wrkspc_e"),
        org: OrgId::new("org_e"),
        queue: queue.into(),
        work,
        last_heartbeat: None,
        lease_until: None,
        handed_out_at: None,
    }
}

#[async_trait::async_trait]
impl WorkReads for Items {
    async fn get(&self, _: &WorkspaceId, session: &str, _: &Reach) -> Result<Option<WorkItem>, StoreError> {
        Ok(match session {
            "sesn_1" => Some(item(session, "hosted", WorkState::Active)),
            "sesn_stopped" => Some(item(session, "hosted", WorkState::Stopped)),
            _ => None,
        })
    }
    async fn find_on_queue(&self, queue: &str, session: &str) -> Result<Option<WorkItem>, StoreError> {
        Ok((queue == "runtime" && session == "sesn_rt").then(|| item(session, queue, WorkState::Active)))
    }
    async fn list(&self, _: &Scope, _: &Pagination) -> Result<Vec<WorkItem>, StoreError> {
        Ok(vec![])
    }
    async fn counts(&self, _: &Scope, _: Duration, _: Timestamp) -> Result<Counts, StoreError> {
        Ok(Counts::default())
    }
    async fn runtime_owner(&self, environment: &str) -> Result<Option<(OrgId, WorkspaceId)>, StoreError> {
        Ok((environment == "env_rt").then(|| (OrgId::new("org_e"), WorkspaceId::new("wrkspc_e"))))
    }
}

struct NoGrants;

#[async_trait::async_trait]
impl SessionGrants for NoGrants {
    async fn grants(&self, _: &AuthContext, _: &str) -> Result<Grants, ApiError> {
        Ok(Grants::default())
    }
}

async fn echo(
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"workspace": ctx.workspace.as_str(), "files_beta": betas.has("files-api-2025-04-14")}),
    )
}

async fn writer() -> Json<serde_json::Value> {
    let writer = Writer::current();
    Json(serde_json::json!({"actor": writer.actor, "request_id": writer.request_id}))
}

async fn create(ApiJson(body): ApiJson<serde_json::Value>) -> Result<Response, RouteError> {
    Ok(axum::response::IntoResponse::into_response(Json(body)))
}

fn app() -> Router {
    let mut keys = StaticKeySource::default();
    keys.register(KEY, "org_e", "wrkspc_e", "apikey_e");
    let mut admin_keys = StaticAdminKeys::default();
    admin_keys.register(ADMIN_KEY, "org_e", "adminkey_e");
    let routes = Router::new()
        .route("/v1/things", get(echo).post(create))
        .route("/v1/sessions/{id}", get(writer))
        .route("/v1/environments/{id}", get(writer))
        .route("/v1/environments/{id}/work/poll", get(writer))
        .route("/v1/environments/{id}/work/{work}", get(writer))
        .route("/v1/environments/{id}/work/{work}/ack", axum::routing::post(writer))
        .route("/v1/organizations/me", get(writer));
    let routes = edge::limits::bounded(routes, BodyLimit { api: "Things", max_mb: 1 });
    edge::wrap(
        routes,
        api::edge::auth::Credentials {
            keys: Arc::new(keys),
            admin_keys: Arc::new(admin_keys),
            environment_keys: Arc::new(Keys),
            work: WorkAccess {
                tokens: work::WorkTokens::new(WORK_KEY),
                items: Arc::new(Items),
                grants: Arc::new(NoGrants),
            },
        },
        Arc::from(vec![BetaGate::managed(&["/v1/things"])]),
        Arc::new(api::telemetry::Metrics::global()),
    )
}

struct Sent {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: serde_json::Value,
}

async fn send(req: Request<Body>) -> Sent {
    let resp = app().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Sent { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null) }
}

fn get_req(path: &str) -> axum::http::request::Builder {
    Request::builder().method("GET").uri(path).header("anthropic-version", "2026-10-21")
}

#[tokio::test]
async fn a_bearer_token_and_x_api_key_both_authenticate() {
    // Live auth/01, 02.
    let bearer = send(
        get_req("/v1/things").header("authorization", format!("Bearer {KEY}")).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(bearer.status, StatusCode::OK);
    assert_eq!(bearer.body["workspace"], "wrkspc_e");
    assert_eq!(bearer.headers["anthropic-organization-id"], "org_e");
    assert_eq!(bearer.headers["anthropic-workspace-id"], "wrkspc_e");
    assert!(bearer.headers["request-id"].to_str().unwrap().starts_with("req_"));
    let key = send(get_req("/v1/things").header("x-api-key", KEY).body(Body::empty()).unwrap()).await;
    assert_eq!(key.status, StatusCode::OK);
}

#[tokio::test]
async fn auth_errors_match_live_and_carry_the_header_request_id() {
    // Live auth/03, 04.
    let none = send(get_req("/v1/things").body(Body::empty()).unwrap()).await;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        none.body["error"],
        serde_json::json!({"message": "x-api-key header is required", "type": "authentication_error"})
    );
    assert_eq!(none.body["request_id"], none.headers["request-id"].to_str().unwrap());
    assert_eq!(none.headers["x-should-retry"], "false");
    let bad =
        send(get_req("/v1/things").header("authorization", "Bearer nope").body(Body::empty()).unwrap()).await;
    assert_eq!(bad.body["error"]["message"], "invalid x-api-key");
}

#[tokio::test]
async fn a_request_runs_as_its_key_or_work_session_under_its_request_id() {
    let as_key =
        send(get_req("/v1/sessions/sesn_1").header("x-api-key", KEY).body(Body::empty()).unwrap()).await;
    assert_eq!(as_key.status, StatusCode::OK);
    assert_eq!(as_key.body["actor"], "apikey_e");
    assert_eq!(as_key.body["request_id"], as_key.headers["request-id"].to_str().unwrap());
    let claims = work::WorkClaims { session: "sesn_1".into(), ws: "wrkspc_e".into(), org: "org_e".into() };
    let token = work::WorkTokens::new(WORK_KEY).mint(&claims);
    let as_work =
        send(get_req("/v1/sessions/sesn_1").header("x-api-key", token).body(Body::empty()).unwrap()).await;
    assert_eq!(as_work.status, StatusCode::OK);
    assert_eq!(as_work.body["actor"], "sesn_1");
    let as_admin =
        send(get_req("/v1/organizations/me").header("x-api-key", ADMIN_KEY).body(Body::empty()).unwrap())
            .await;
    assert_eq!(as_admin.status, StatusCode::OK);
    assert_eq!(as_admin.body["actor"], "adminkey_e");
    assert_eq!(as_admin.body["request_id"], as_admin.headers["request-id"].to_str().unwrap());
}

#[tokio::test]
async fn anthropic_version_is_required() {
    // Live auth/05.
    let req = Request::builder().uri("/v1/things").header("x-api-key", KEY).body(Body::empty()).unwrap();
    let sent = send(req).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert_eq!(sent.body["error"]["message"], "anthropic-version: header is required");
}

#[tokio::test]
async fn betas_are_read_from_every_header_and_unknown_ones_are_400() {
    // Live auth/07.
    let sent = send(
        get_req("/v1/things")
            .header("x-api-key", KEY)
            .header("anthropic-beta", "not-a-beta-2020-01-01")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        sent.body["error"]["message"],
        "Unexpected value(s) `not-a-beta-2020-01-01` for the `anthropic-beta` header. Please consult our \
         documentation at platform.claude.com/docs or try again without the header."
    );
    assert_eq!(sent.headers["anthropic-organization-id"], "org_e");
    let ok = send(
        get_req("/v1/things")
            .header("x-api-key", KEY)
            .header("anthropic-beta", "managed-agents-2026-04-01")
            .header("anthropic-beta", "files-api-2025-04-14")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(ok.body["files_beta"], true);
}

#[tokio::test]
async fn a_gated_route_without_its_beta_reads_the_ga_schema() {
    let req = Request::builder()
        .method("POST")
        .uri("/v1/things")
        .header("x-api-key", KEY)
        .header("anthropic-version", "2026-10-21")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"name":"a","model":"m"}"#))
        .unwrap();
    let sent = send(req).await;
    assert_eq!(sent.status, StatusCode::BAD_REQUEST);
    assert_eq!(sent.body["error"]["message"], "Failed to parse request body: unknown field \"model\"");
}

#[tokio::test]
async fn an_unknown_path_is_the_live_404_without_auth_or_request_id() {
    // Live auth/08.
    let sent = send(get_req("/v1/not_a_route").body(Body::empty()).unwrap()).await;
    assert_eq!(sent.status, StatusCode::NOT_FOUND);
    assert_eq!(
        sent.body,
        serde_json::json!({"error": {"message": "Not found", "type": "not_found_error"}, "request_id": null, "type": "error"})
    );
    assert!(sent.headers.get("request-id").is_none());
    assert_eq!(sent.headers["x-should-retry"], "false");
}

#[tokio::test]
async fn unserved_methods_and_head_are_405() {
    // Live files/15, 16.
    let put =
        send(get_req("/v1/things").method("PUT").header("x-api-key", KEY).body(Body::empty()).unwrap()).await;
    assert_eq!(put.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        put.body["error"],
        serde_json::json!({"message": "Method Not Allowed", "type": "invalid_request_error"})
    );
    assert_eq!(put.headers.get("allow").unwrap(), "GET");
    assert!(put.headers.get("anthropic-organization-id").is_none());
    let head =
        send(get_req("/v1/things").method("HEAD").header("x-api-key", KEY).body(Body::empty()).unwrap())
            .await;
    assert_eq!(head.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(head.headers["allow"], "GET");
    assert_eq!(head.body, serde_json::Value::Null);
}

#[tokio::test]
async fn a_body_over_the_cap_is_the_live_413() {
    // Live skills/13.
    let big = vec![b'a'; 1024 * 1024 + 1];
    let req = Request::builder()
        .method("POST")
        .uri("/v1/things")
        .header("x-api-key", KEY)
        .header("anthropic-version", "2026-10-21")
        .header("anthropic-beta", "managed-agents-2026-04-01")
        .header("content-type", "application/json")
        .header("content-length", big.len())
        .body(Body::from(big))
        .unwrap();
    let sent = send(req).await;
    assert_eq!(sent.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        sent.body,
        serde_json::json!({"error": {"message": "Request exceeds the maximum size. The Things API accepts requests up to 1MBs.", "type": "request_too_large"}})
    );
    assert!(sent.headers.get("request-id").is_none());
}

fn with_key(path: &str, header: &str, value: &str) -> Request<Body> {
    get_req(path).header(header, value).body(Body::empty()).unwrap()
}

fn bearer(path: &str, key: &str) -> Request<Body> {
    with_key(path, "authorization", &format!("Bearer {key}"))
}

#[tokio::test]
async fn an_environment_key_is_a_bearer_token_only() {
    // W-15: live answers an environment key sent as x-api-key with 401.
    let sent = send(with_key("/v1/environments/env_e/work/poll", "x-api-key", ENV_KEY)).await;
    assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        sent.body["error"],
        serde_json::json!({"message": "invalid x-api-key", "type": "authentication_error"})
    );
    let ok = send(bearer("/v1/environments/env_e/work/poll", ENV_KEY)).await;
    assert_eq!((ok.status, ok.body["actor"].as_str()), (StatusCode::OK, Some("envkey_e")));
}

#[tokio::test]
async fn an_environment_key_off_its_routes_gets_live_s_scope_403() {
    // W-16: reading the environment, one work item, another queue, or any other resource.
    for path in [
        "/v1/environments/env_e",
        "/v1/environments/env_e/work/sesn_1",
        "/v1/environments/env_x/work/poll",
        "/v1/things",
    ] {
        let sent = send(bearer(path, ENV_KEY)).await;
        assert_eq!(sent.status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(
            sent.body["error"],
            serde_json::json!({"message": SCOPE, "type": "permission_error"}),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_workspace_key_on_poll_is_no_bearer_token() {
    // W-18.
    for sent in [
        send(with_key("/v1/environments/env_e/work/poll", "x-api-key", KEY)).await,
        send(bearer("/v1/environments/env_e/work/poll", KEY)).await,
    ] {
        assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
        assert_eq!(
            sent.body["error"],
            serde_json::json!({"message": "Invalid bearer token", "type": "authentication_error"})
        );
    }
    let unknown = send(bearer("/v1/environments/env_e/work/poll", "sk-ant-oat01-unknown")).await;
    assert_eq!(unknown.body["error"]["message"], "Invalid bearer token");
}

#[tokio::test]
async fn the_runtime_key_reads_only_an_environment_with_runtime_work() {
    // W-22: it runs in the workspace of that environment's runtime item.
    let served = send(bearer("/v1/environments/env_rt", RUNTIME_KEY)).await;
    assert_eq!((served.status, served.body["actor"].as_str()), (StatusCode::OK, Some("runtime")));
    assert_eq!(served.headers["anthropic-workspace-id"], "wrkspc_e");
    // W-23, and an environment key on its own environment.
    for sent in [
        send(bearer("/v1/environments/env_none", RUNTIME_KEY)).await,
        send(bearer("/v1/environments/env_e", ENV_KEY)).await,
    ] {
        assert_eq!(sent.status, StatusCode::FORBIDDEN);
        assert_eq!(sent.body["error"]["message"], SCOPE);
    }
    let poll = send(bearer("/v1/environments/runtime/work/poll", RUNTIME_KEY)).await;
    assert_eq!(poll.status, StatusCode::OK);
    let elsewhere = send(bearer("/v1/environments/env_e/work/poll", RUNTIME_KEY)).await;
    assert_eq!(elsewhere.status, StatusCode::FORBIDDEN);
    let off_queue = send(bearer("/v1/sessions/sesn_1", RUNTIME_KEY)).await;
    assert_eq!(
        (off_queue.status, off_queue.body["error"].clone()),
        (
            StatusCode::NOT_FOUND,
            serde_json::json!({"message": "session not found", "type": "not_found_error"})
        )
    );
    let off_work = send(bearer("/v1/environments/runtime/work/sesn_1", RUNTIME_KEY)).await;
    assert_eq!(
        off_work.status,
        StatusCode::FORBIDDEN,
        "reading one work item is off an environment key's routes"
    );
    let ack = Request::builder()
        .method("POST")
        .uri("/v1/environments/runtime/work/sesn_1/ack")
        .header("anthropic-version", "2026-10-21")
        .header("authorization", format!("Bearer {RUNTIME_KEY}"))
        .body(Body::empty())
        .unwrap();
    let off_ack = send(ack).await;
    assert_eq!(
        (off_ack.status, off_ack.body["error"].clone()),
        (StatusCode::NOT_FOUND, serde_json::json!({"message": "work not found", "type": "not_found_error"}))
    );
    let on_queue = send(bearer("/v1/sessions/sesn_rt", RUNTIME_KEY)).await;
    assert_eq!((on_queue.status, on_queue.body["actor"].as_str()), (StatusCode::OK, Some("runtime")));
}

#[tokio::test]
async fn a_work_token_is_refused_once_its_item_is_stopped() {
    // L-12.
    let tokens = work::WorkTokens::new(WORK_KEY);
    let mint = |s: &str| {
        tokens.mint(&work::WorkClaims { session: s.into(), ws: "wrkspc_e".into(), org: "org_e".into() })
    };
    let live = send(bearer("/v1/sessions/sesn_1", &mint("sesn_1"))).await;
    assert_eq!(live.status, StatusCode::OK);
    for token in [mint("sesn_stopped"), mint("sesn_gone")] {
        let sent = send(bearer("/v1/sessions/sesn_stopped", &token)).await;
        assert_eq!(sent.status, StatusCode::UNAUTHORIZED);
        assert_eq!(sent.body["error"]["type"], "authentication_error");
    }
    let other = send(bearer("/v1/sessions/sesn_2", &mint("sesn_1"))).await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED, "another session's route");
}
