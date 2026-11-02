//! Authentication: the API key from `x-api-key` or `Authorization: Bearer`.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use shared::ports::keys::KeySource;
use shared::{wire_response, ApiError, AuthContext, OrgId, WorkspaceId};
use work::{WorkClaims, WorkScope, WorkTokens};

/// What a request may authenticate with: an API key, or a sandbox
/// worker's work token.
#[derive(Clone)]
pub struct Credentials {
    /// API keys.
    pub keys: Arc<dyn KeySource>,
    /// Work tokens, when sandboxes run workers.
    pub work: Option<WorkTokens>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("work", &self.work.is_some()).finish_non_exhaustive()
    }
}

/// The routes a work token reaches: its own work item's, nothing else.
fn work_route(path: &str, claims: &WorkClaims) -> bool {
    let mut parts = path.trim_start_matches('/').split('/');
    let fixed = [Some("v1"), Some("environments")];
    let head_ok = fixed.iter().all(|want| parts.next() == *want);
    let _env = parts.next();
    head_ok && parts.next() == Some("work") && parts.next() == Some(claims.work.as_str())
}

/// Resolve the caller's key or work token to its tenant. Live (auth/03, 04): no key is
/// `x-api-key header is required`, an unknown one `invalid x-api-key`, both
/// 401 `authentication_error`. A key store that cannot be read is a 500, so a
/// client retries instead of taking its key for revoked.
pub async fn auth(State(creds): State<Credentials>, mut req: Request, next: Next) -> Response {
    let Some(key) = api_key(req.headers()) else {
        return wire_response(&ApiError::authentication("x-api-key header is required"));
    };
    let ctx = if key.starts_with(work::tokens::PREFIX) {
        let claims = creds.work.as_ref().and_then(|w| w.verify(&key));
        let Some(claims) = claims.filter(|c| work_route(req.uri().path(), c)) else {
            return wire_response(&ApiError::authentication("invalid work token"));
        };
        let ctx = AuthContext {
            org: OrgId::new(claims.org.clone()),
            workspace: WorkspaceId::new(claims.ws.clone()),
            api_key_id: format!("work:{}", claims.work),
        };
        req.extensions_mut().insert(WorkScope(claims));
        ctx
    } else {
        match creds.keys.resolve(&key).await {
            Ok(Some(ctx)) => ctx,
            Ok(None) => return wire_response(&ApiError::authentication("invalid x-api-key")),
            Err(e) => return wire_response(&ApiError::internal(e.to_string())),
        }
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

    #[test]
    fn a_work_token_reaches_only_its_own_work_routes() {
        let claims =
            WorkClaims { work: "work_1".into(), session: "s".into(), ws: "w".into(), org: "o".into() };
        assert!(work_route("/v1/environments/env_1/work/work_1/ops", &claims));
        assert!(work_route("/v1/environments/env_1/work/work_1", &claims));
        assert!(!work_route("/v1/environments/env_1/work/work_2/ops", &claims));
        assert!(!work_route("/v1/environments/env_1/work/poll", &claims));
        assert!(!work_route("/v1/agents", &claims));
        assert!(!work_route("/v1/sessions/sesn_1/events", &claims));
    }
}
