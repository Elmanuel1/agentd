//! Live answers for requests no route serves.

use axum::extract::Request;
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use shared::{wire_response, ApiError};

/// Live (files/16, files/17, skills/46): a method the path does not serve is
/// 405 with an envelope and `allow: GET`, even on a route that also serves
/// DELETE (axum's own `Allow` would list every method, HEAD included).
pub async fn not_allowed() -> Response {
    let mut e = ApiError::invalid_request("Method Not Allowed");
    e.status = 405;
    let mut response = wire_response(&e);
    response.headers_mut().insert("allow", HeaderValue::from_static("GET"));
    response
}

/// Live (files/15): HEAD is not served, even on a GET route: 405, `allow: GET`,
/// no body.
pub async fn reject_head(req: Request, next: Next) -> Response {
    if req.method() != Method::HEAD {
        return next.run(req).await;
    }
    let mut response =
        (StatusCode::METHOD_NOT_ALLOWED, [("content-type", "application/json")]).into_response();
    response.headers_mut().insert("allow", HeaderValue::from_static("GET"));
    response.headers_mut().insert("x-should-retry", HeaderValue::from_static("false"));
    response
}
