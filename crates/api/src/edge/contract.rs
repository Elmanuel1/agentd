//! The request contract: `anthropic-version` and `anthropic-beta`.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use shared::beta::{ApiVersion, BetaGate, BetaSet};
use shared::module::BodyLimit;
use shared::{request_too_large_response, wire_response, ApiError};

/// Live (auth/05): a request without `anthropic-version` is a 400.
pub async fn version(mut req: Request, next: Next) -> Response {
    let Some(version) = req.headers().get("anthropic-version").and_then(|v| v.to_str().ok()) else {
        return wire_response(&ApiError::invalid_request("anthropic-version: header is required"));
    };
    let version = ApiVersion(version.to_owned());
    req.extensions_mut().insert(version);
    next.run(req).await
}

/// Parse every `anthropic-beta` header (live auth/07: an unknown name is a
/// 400), then apply the gate of the route's resource.
pub async fn betas(State(gates): State<Arc<[BetaGate]>>, mut req: Request, next: Next) -> Response {
    let values = req.headers().get_all("anthropic-beta").iter().filter_map(|v| v.to_str().ok());
    let betas = match BetaSet::parse(values) {
        Ok(betas) => betas,
        Err(unknown) => {
            let names = unknown.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ");
            return wire_response(&ApiError::invalid_request(format!(
                "Unexpected value(s) {names} for the `anthropic-beta` header. Please consult our \
                 documentation at platform.claude.com/docs or try again without the header."
            )));
        }
    };
    let gate = gates.iter().find(|g| g.covers(req.uri().path())).copied();
    let bodied = matches!(req.method().as_str(), "POST" | "PUT" | "PATCH");
    let ungated = gate.filter(|g| bodied && !g.opened_by(&betas));
    req.extensions_mut().insert(betas);
    match ungated {
        Some(gate) => ga_schema(gate, req, next).await,
        None => next.run(req).await,
    }
}

/// Beta-gated routes are Managed Agents and memory routes, whose body cap is
/// 32 MB; the GA-schema check reads no more than that.
const GATED: BodyLimit = BodyLimit { api: "Managed Agents", max_mb: 32 };

/// Without its beta, a gated route's body is read against the GA schema:
/// the first field the GA schema does not know is the live 400
/// `Failed to parse request body: unknown field "model"`.
async fn ga_schema(gate: BetaGate, req: Request, next: Next) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, GATED.bytes()).await else {
        return request_too_large_response(GATED.api, GATED.max_mb);
    };
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        if let Some(unknown) = map.keys().find(|k| !gate.ga_fields.contains(&k.as_str())) {
            return wire_response(&ApiError::invalid_request(format!(
                "Failed to parse request body: unknown field \"{unknown}\""
            )));
        }
    }
    next.run(Request::from_parts(parts, Body::from(bytes))).await
}
