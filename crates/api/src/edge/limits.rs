//! Each module's request body cap.

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::StatusCode;
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use axum::Router;
use shared::module::BodyLimit;
use shared::request_too_large_response;

/// `routes` with bodies above `limit` answered by the live 413.
pub fn bounded(routes: Router, limit: BodyLimit) -> Router {
    routes.layer(DefaultBodyLimit::max(limit.bytes())).layer(from_fn_with_state(limit, cap))
}

/// A declared length above the cap is refused before the body is read; a
/// body that runs over it while being read is refused by the extractor, and
/// either way the answer is the live 413.
async fn cap(State(limit): State<BodyLimit>, req: Request, next: Next) -> Response {
    let declared = req
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit.bytes()) {
        return request_too_large_response(limit.api, limit.max_mb);
    }
    let response = next.run(req).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return request_too_large_response(limit.api, limit.max_mb);
    }
    response
}
