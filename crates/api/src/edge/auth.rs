//! Authentication: the API key from `x-api-key` or `Authorization: Bearer`.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use shared::ports::keys::KeySource;
use shared::{wire_response, ApiError, AuthContext};

/// Resolve the caller's key to its tenant. Live (auth/03, 04): no key is
/// `x-api-key header is required`, an unknown one `invalid x-api-key`, both
/// 401 `authentication_error`. A key store that cannot be read is a 500, so a
/// client retries instead of taking its key for revoked.
pub async fn auth(State(keys): State<Arc<dyn KeySource>>, mut req: Request, next: Next) -> Response {
    let Some(key) = api_key(req.headers()) else {
        return wire_response(&ApiError::authentication("x-api-key header is required"));
    };
    let ctx = match keys.resolve(&key).await {
        Ok(Some(ctx)) => ctx,
        Ok(None) => return wire_response(&ApiError::authentication("invalid x-api-key")),
        Err(e) => return wire_response(&ApiError::internal(e.to_string())),
    };
    let tenant = [
        ("anthropic-organization-id", HeaderValue::from_str(ctx.org.as_str()).ok()),
        ("anthropic-workspace-id", HeaderValue::from_str(ctx.workspace.as_str()).ok()),
    ];
    req.extensions_mut().insert::<AuthContext>(ctx);
    let mut response = next.run(req).await;
    // Live (files/16): a 405 is answered by routing and names no tenant.
    if response.status() != StatusCode::METHOD_NOT_ALLOWED {
        for (name, value) in tenant {
            if let Some(value) = value {
                response.headers_mut().insert(name, value);
            }
        }
    }
    response
}

fn api_key(headers: &HeaderMap) -> Option<String> {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(key.to_owned());
    }
    let auth = headers.get("authorization")?.to_str().ok()?;
    let (scheme, token) = auth.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_comes_from_x_api_key_or_a_bearer_token() {
        let mut h = HeaderMap::new();
        h.insert("authorization", HeaderValue::from_static("Bearer sk-1"));
        assert_eq!(api_key(&h).as_deref(), Some("sk-1"));
        h.insert("x-api-key", HeaderValue::from_static("sk-2"));
        assert_eq!(api_key(&h).as_deref(), Some("sk-2"));
        let mut basic = HeaderMap::new();
        basic.insert("authorization", HeaderValue::from_static("Basic abc"));
        assert_eq!(api_key(&basic), None);
    }
}
