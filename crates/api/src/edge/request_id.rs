//! One request id per request: in the `request-id` header and in every error
//! envelope the request renders.

use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use shared::{with_request_id, NoRequestId, RequestId};

/// Mint the request's id, run it, and stamp the id on the response.
pub async fn request_id(mut req: Request, next: Next) -> Response {
    let id = RequestId::generate();
    req.extensions_mut().insert(id.clone());
    let header = HeaderValue::from_str(id.as_str()).ok();
    let mut response = with_request_id(id, next.run(req)).await;
    if response.extensions().get::<NoRequestId>().is_none() {
        if let Some(value) = header {
            response.headers_mut().insert("request-id", value);
        }
    }
    response
}
