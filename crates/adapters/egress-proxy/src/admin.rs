//! The admin interface agentd registers sessions through. It takes a bearer
//! token only agentd holds, and listens on its own address, which the
//! deployment puts on a network only agentd shares. It serves no request
//! data: no headers, no metrics, no debug routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use shared::charset::is_id_charset;

use crate::registry::{Registration, Registry};

#[derive(Clone)]
struct Admin {
    registry: Arc<Registry>,
    token: [u8; 32],
}

/// The admin routes over `registry`, for callers holding `token`.
pub fn router(registry: Arc<Registry>, token: &SecretString) -> Router {
    let token = Sha256::digest(token.expose_secret().as_bytes()).into();
    Router::new()
        .route("/v1/sessions/{id}", put(register).delete(forget))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(Admin { registry, token })
}

/// Whether `headers` carry the admin token. Digests are compared, so the
/// time taken says nothing about the token.
fn authorized(admin: &Admin, headers: &HeaderMap) -> bool {
    let given = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    given.is_some_and(|t| <[u8; 32]>::from(Sha256::digest(t.as_bytes())) == admin.token)
}

async fn register(
    State(admin): State<Admin>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // The token first: nothing about the request is read for a caller without it.
    if !authorized(&admin, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !is_id_charset(&id) {
        return (StatusCode::BAD_REQUEST, "invalid session id").into_response();
    }
    let reg: Registration = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    };
    match admin.registry.register(&id, reg) {
        Ok(done) => Json(done).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

async fn forget(State(admin): State<Admin>, Path(id): Path<String>, headers: HeaderMap) -> StatusCode {
    if !authorized(&admin, &headers) {
        return StatusCode::UNAUTHORIZED;
    }
    admin.registry.forget(&id);
    StatusCode::NO_CONTENT
}
