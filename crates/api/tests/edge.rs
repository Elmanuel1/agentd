//! The HTTP edge against the live captures (`fixtures/live/2026-11-01/auth`,
//! `files/15-17`, `skills/13`): hermetic, over a stand-in module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use api::edge;
use api::EnvKeySource;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use shared::beta::{BetaGate, BetaSet};
use shared::module::BodyLimit;
use shared::{ApiJson, AuthContext, RouteError};
use tower::ServiceExt;

const KEY: &str = "sk-edge";

async fn echo(
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
) -> Json<serde_json::Value> {
    Json(
        serde_json::json!({"workspace": ctx.workspace.as_str(), "files_beta": betas.has("files-api-2025-04-14")}),
    )
}

async fn create(ApiJson(body): ApiJson<serde_json::Value>) -> Result<Response, RouteError> {
    Ok(axum::response::IntoResponse::into_response(Json(body)))
}

fn app() -> Router {
    let mut keys = EnvKeySource::default();
    keys.register(KEY, "org_e", "wrkspc_e", "apikey_e");
    let routes = Router::new().route("/v1/things", get(echo).post(create));
    let routes = edge::limits::bounded(routes, BodyLimit { api: "Things", max_mb: 1 });
    edge::wrap(
        routes,
        Arc::new(keys),
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
