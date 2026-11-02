//! `ApiJson<T>` — a request-body JSON extractor whose rejection renders
//! agentd's wire error envelope with live-shaped messages.
//!
//! Live (probed 2026-07-05): a body that fails validation returns
//! `{"type":"error","error":{"type":"invalid_request_error","message":…}}`
//! where a missing required field is `"X: Field required"` and an unrecognized
//! field is `Failed to parse request body: unknown field "X"`. Request types
//! opt into unknown-field rejection with `#[serde(deny_unknown_fields)]`.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::error::{ApiError, ErrorType};
use crate::id::RequestId;

/// Drop-in replacement for `axum::Json<T>` on request bodies whose extraction
/// failures match the live Anthropic error wire format.
#[derive(Debug, Clone, Copy)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiJsonError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(ApiJsonError(map_rejection(&rejection))),
        }
    }
}

tokio::task_local! {
    static REQUEST_ID: RequestId;
}

/// Run `fut` as the request `id`: every error envelope it renders carries `id`.
pub async fn with_request_id<F: std::future::Future>(id: RequestId, fut: F) -> F::Output {
    REQUEST_ID.scope(id, fut).await
}

/// Marks a response that carries no `request-id` header (live: an unknown
/// route answers with `request_id: null` and no header).
#[derive(Debug, Clone, Copy)]
pub struct NoRequestId;

/// Render an `ApiError` as the wire envelope, carrying the current request's
/// id (routes, extractor rejections, and the edge layers all go through here).
/// A 4xx other than 408 and 429 carries `x-should-retry: false`, as live does.
pub fn wire_response(error: &ApiError) -> Response {
    let rid = REQUEST_ID.try_with(Clone::clone).unwrap_or_else(|_| RequestId::generate());
    render(error, Some(&rid))
}

/// The 404 for a path no route serves: `request_id: null`, no `request-id` header.
pub fn unknown_route_response() -> Response {
    let mut response = render(&ApiError::not_found("Not found"), None);
    response.extensions_mut().insert(NoRequestId);
    response
}

/// The 413 for a body over a router's cap. Live (skills/13) answers it before
/// the API runs: `{"error":{"message","type":"request_too_large"}}`, with no
/// request id and no `x-should-retry`.
pub fn request_too_large_response(api: &str, max_mb: usize) -> Response {
    let body = serde_json::json!({"error": {
        "message": format!("Request exceeds the maximum size. The {api} API accepts requests up to {max_mb}MBs."),
        "type": "request_too_large",
    }});
    let mut response = (axum::http::StatusCode::PAYLOAD_TOO_LARGE, Json(body)).into_response();
    response.extensions_mut().insert(NoRequestId);
    response
}

/// A 5xx's own message, kept off the wire (which says only `Internal server
/// error` or `Overloaded`, as live does) and on the response for the edge to
/// log.
#[derive(Debug, Clone)]
pub struct ErrorCause(pub String);

fn render(error: &ApiError, rid: Option<&RequestId>) -> Response {
    let status = axum::http::StatusCode::from_u16(error.status)
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_server_error() {
        let public = match error.error_type {
            ErrorType::Overloaded => "Overloaded",
            _ => "Internal server error",
        };
        let shown = ApiError { message: public.into(), ..error.clone() };
        let mut response = (status, Json(shown.to_envelope(rid))).into_response();
        response.extensions_mut().insert(ErrorCause(error.message.clone()));
        return response;
    }
    let mut response = (status, Json(error.to_envelope(rid))).into_response();
    if status.is_client_error() && !matches!(error.status, 408 | 429) {
        response.headers_mut().insert("x-should-retry", axum::http::HeaderValue::from_static("false"));
    }
    response
}

/// The rejection type — renders the wire envelope with a fresh request id.
#[derive(Debug)]
pub struct ApiJsonError(pub ApiError);

impl IntoResponse for ApiJsonError {
    fn into_response(self) -> Response {
        wire_response(&self.0)
    }
}

/// The canonical route error wrapper: lets handlers return
/// `Result<Response, RouteError>` and `?`-propagate `ApiError`, rendering the
/// shared wire envelope. Replaces the byte-identical copy every entity crate
/// used to define (the orphan rule forbids `impl IntoResponse for ApiError`
/// downstream, which is exactly why this lives in core).
#[derive(Debug)]
pub struct RouteError(pub ApiError);

impl From<ApiError> for RouteError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        wire_response(&self.0)
    }
}

/// A multipart body that could not be read: over the body cap is a 413, any
/// other failure a 400.
pub fn multipart_error(e: &axum::extract::multipart::MultipartError) -> ApiError {
    if e.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::request_too_large("Request exceeds the maximum size.");
    }
    ApiError::invalid_request("Failed to parse request body")
}

fn map_rejection(rejection: &JsonRejection) -> ApiError {
    if rejection.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
        return ApiError::request_too_large("Request exceeds the maximum size.");
    }
    let text = rejection.body_text();
    if let Some(field) = between(&text, "missing field `", "`") {
        return ApiError::invalid_request(format!("{field}: Field required"));
    }
    if let Some(field) = between(&text, "unknown field `", "`") {
        return ApiError::invalid_request(format!("Failed to parse request body: unknown field \"{field}\""));
    }
    ApiError::invalid_request("Failed to parse request body")
}

/// Slice out the text between `start` and the next `end`, allocation-free and
/// index-safe.
fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let after = text.split_once(start)?.1;
    Some(after.split_once(end)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_field_name() {
        assert_eq!(between("missing field `version` at line 1", "missing field `", "`"), Some("version"));
        assert_eq!(between("unknown field `bogus`, expected", "unknown field `", "`"), Some("bogus"));
    }

    #[tokio::test]
    async fn a_server_error_keeps_its_cause_off_the_wire() {
        let response = wire_response(&ApiError::internal("store backend: password=hunter2 refused"));
        assert_eq!(
            response.extensions().get::<ErrorCause>().map(|c| c.0.as_str()),
            Some("store backend: password=hunter2 refused")
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["type"], "api_error");
        assert_eq!(body["error"]["message"], "Internal server error");
        let client = wire_response(&ApiError::invalid_request("name: too long"));
        assert!(client.extensions().get::<ErrorCause>().is_none());
    }
}
