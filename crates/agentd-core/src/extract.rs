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

use crate::error::ApiError;
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

/// Render an `ApiError` as the wire envelope with a fresh request id — the one
/// place this happens (routes, extractor rejections, and the 404 fallback all
/// go through here).
pub fn wire_response(error: &ApiError) -> Response {
    let rid = RequestId::generate();
    let status = axum::http::StatusCode::from_u16(error.status)
        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    (status, Json(error.to_envelope(&rid))).into_response()
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

fn map_rejection(rejection: &JsonRejection) -> ApiError {
    let text = rejection.body_text();
    if let Some(field) = between(&text, "missing field `", "`") {
        return ApiError::invalid_request(format!("{field}: Field required"));
    }
    if let Some(field) = between(&text, "unknown field `", "`") {
        return ApiError::invalid_request(format!(
            "Failed to parse request body: unknown field \"{field}\""
        ));
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
        assert_eq!(
            between("missing field `version` at line 1", "missing field `", "`"),
            Some("version")
        );
        assert_eq!(
            between("unknown field `bogus`, expected", "unknown field `", "`"),
            Some("bogus")
        );
    }
}
