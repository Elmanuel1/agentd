//! Wire-compat middleware: auth, the beta-header gate, and the 404 fallback.

use agentd_core::{ApiError, AuthContext, RequestId};
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::state::ApiKeyRegistry;

/// The managed-agents beta header value (live).
pub const MANAGED_AGENTS_BETA: &str = "managed-agents-2026-04-01";

fn envelope(e: &ApiError) -> Response {
    let rid = RequestId::generate();
    let status =
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(e.to_envelope(&rid))).into_response()
}

/// `x-api-key` → [`AuthContext`]. Missing/unknown → 401 envelope
/// (`authentication_error`), matching the live error family.
pub async fn auth(
    State(keys): State<ApiKeyRegistry>,
    mut req: Request,
    next: Next,
) -> Response {
    let key = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let Some(key) = key else {
        return envelope(&ApiError::authentication(
            "missing x-api-key header",
        ));
    };
    let Some(ctx) = keys.resolve(&key) else {
        return envelope(&ApiError::authentication("invalid x-api-key"));
    };
    req.extensions_mut().insert::<AuthContext>(ctx);
    next.run(req).await
}

/// The beta-header gate. Live behavior (probed): a bodied managed-agents
/// request WITHOUT `anthropic-beta: managed-agents-2026-04-01` is parsed
/// against a non-beta schema, so beta-only fields reject with
/// `Failed to parse request body: unknown field "model"` (400).
///
/// The non-beta schema knows only generic fields; the first beta-only key in
/// the body names the error, reproducing the observed message.
pub async fn beta_gate(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    let has_beta = req
        .headers()
        .get("anthropic-beta")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|b| b.trim() == MANAGED_AGENTS_BETA));
    let is_managed_route = path.starts_with("/v1/agents")
        || path.starts_with("/v1/environments")
        || path.starts_with("/v1/sessions")
        || path.starts_with("/v1/vaults")
        || path.starts_with("/v1/memory_stores")
        || path.starts_with("/v1/deployments")
        || path.starts_with("/v1/deployment_runs");
    let is_bodied = matches!(
        req.method().as_str(),
        "POST" | "PUT" | "PATCH"
    );
    if !is_managed_route || has_beta || !is_bodied {
        return next.run(req).await;
    }
    // Reparse the body against the "non-beta schema": generic fields pass,
    // the first beta-only field rejects with the live message.
    const NON_BETA_FIELDS: &[&str] = &["name", "description", "metadata"];
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => {
            return envelope(&ApiError::invalid_request("Failed to parse request body"))
        }
    };
    if let Ok(serde_json::Value::Object(map)) =
        serde_json::from_slice::<serde_json::Value>(&bytes)
    {
        if let Some(unknown) = map.keys().find(|k| !NON_BETA_FIELDS.contains(&k.as_str())) {
            return envelope(&ApiError::invalid_request(format!(
                "Failed to parse request body: unknown field \"{unknown}\""
            )));
        }
    }
    let req = Request::from_parts(parts, Body::from(bytes));
    next.run(req).await
}

/// Unknown route → the wire 404 envelope (never a bare axum 404).
pub async fn not_found() -> Response {
    envelope(&ApiError::not_found("Not Found"))
}
