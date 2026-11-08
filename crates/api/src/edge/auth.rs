//! Authentication: the API key from `x-api-key` or `Authorization: Bearer`.
//! The route decides which store is asked: `/v1/organizations` routes take
//! admin keys only, every other route workspace keys and work tokens only.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use shared::ports::keys::{AdminKeySource, KeySource};
use shared::{
    current_request_id, wire_response, with_writer, ApiError, AuthContext, OrgId, SessionCaller, WorkspaceId,
    Writer,
};
use work::{WorkClaims, WorkScope, WorkTokens};

/// What a request may authenticate with: a workspace key, an admin key, or
/// a sandbox worker's work token.
#[derive(Clone)]
pub struct Credentials {
    /// Workspace keys.
    pub keys: Arc<dyn KeySource>,
    /// Admin keys.
    pub admin_keys: Arc<dyn AdminKeySource>,
    /// Work tokens, when sandboxes run workers.
    pub work: Option<WorkTokens>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("work", &self.work.is_some()).finish_non_exhaustive()
    }
}

/// The routes a work token reaches: its own work item's; its own session's
/// reads, and its events, where it sends the agent's side; reads of the
/// workspace's files and skills, which the harness mounts; and the files
/// routes the harness saves its outputs through. Nothing else.
fn work_route(method: &Method, path: &str, claims: &WorkClaims) -> bool {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let read = method == Method::GET;
    match parts.as_slice() {
        ["v1", "environments", _, "work", work, ..] => *work == claims.work,
        ["v1", "sessions", session] | ["v1", "sessions", session, "events", "stream"] => {
            read && *session == claims.session
        }
        ["v1", "sessions", session, "events"] => {
            (read || method == Method::POST) && *session == claims.session
        }
        ["v1", "files"] => method == Method::POST,
        ["v1", "files", _] => read || method == Method::DELETE,
        ["v1", "files", _, "content"] => read,
        ["v1", "skills", ..] => read,
        _ => false,
    }
}

fn admin_route(path: &str) -> bool {
    path == "/v1/organizations" || path.starts_with("/v1/organizations/")
}

/// Resolve the caller's key or work token to its tenant. Live (auth/03, 04): no key is
/// `x-api-key header is required`, an unknown one `invalid x-api-key`, both
/// 401 `authentication_error`. A key store that cannot be read is a 500, so a
/// client retries instead of taking its key for revoked. The request then
/// runs as its writer: the key's id, or a work token's session.
pub async fn auth(State(creds): State<Credentials>, mut req: Request, next: Next) -> Response {
    let Some(key) = api_key(req.headers()) else {
        return wire_response(&ApiError::authentication("x-api-key header is required"));
    };
    if admin_route(req.uri().path()) {
        let ctx = match creds.admin_keys.resolve(&key).await {
            Ok(Some(ctx)) => ctx,
            Ok(None) => return wire_response(&ApiError::authentication("invalid x-api-key")),
            Err(e) => return wire_response(&ApiError::internal(e.to_string())),
        };
        let tenant = [("anthropic-organization-id", HeaderValue::from_str(ctx.org.as_str()).ok())];
        let actor = ctx.admin_key_id.clone();
        req.extensions_mut().insert(ctx);
        return with_tenant(run_as(actor, req, next).await, tenant);
    }
    let (ctx, actor) = if key.starts_with(work::tokens::PREFIX) {
        let claims = creds.work.as_ref().and_then(|w| w.verify(&key));
        let Some(claims) = claims.filter(|c| work_route(req.method(), req.uri().path(), c)) else {
            return wire_response(&ApiError::authentication("invalid work token"));
        };
        let ctx = AuthContext {
            org: OrgId::new(claims.org.clone()),
            workspace: WorkspaceId::new(claims.ws.clone()),
            api_key_id: format!("work:{}", claims.work),
        };
        let session = claims.session.clone();
        req.extensions_mut().insert(SessionCaller { session: session.clone() });
        req.extensions_mut().insert(WorkScope(claims));
        (ctx, session)
    } else {
        match creds.keys.resolve(&key).await {
            Ok(Some(ctx)) => {
                let actor = ctx.api_key_id.clone();
                (ctx, actor)
            }
            Ok(None) => return wire_response(&ApiError::authentication("invalid x-api-key")),
            Err(e) => return wire_response(&ApiError::internal(e.to_string())),
        }
    };
    let tenant = [
        ("anthropic-organization-id", HeaderValue::from_str(ctx.org.as_str()).ok()),
        ("anthropic-workspace-id", HeaderValue::from_str(ctx.workspace.as_str()).ok()),
    ];
    req.extensions_mut().insert::<AuthContext>(ctx);
    with_tenant(run_as(actor, req, next).await, tenant)
}

async fn run_as(actor: String, req: Request, next: Next) -> Response {
    with_writer(Writer::new(actor, &current_request_id()), next.run(req)).await
}

fn with_tenant<const N: usize>(
    mut response: Response,
    tenant: [(&'static str, Option<HeaderValue>); N],
) -> Response {
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
    fn only_organizations_routes_are_admin_routes() {
        for admin in ["/v1/organizations", "/v1/organizations/me", "/v1/organizations/workspaces/w"] {
            assert!(admin_route(admin), "{admin}");
        }
        for other in ["/v1/agents", "/v1/organizationsx", "/v1/environments/organizations/me"] {
            assert!(!admin_route(other), "{other}");
        }
    }

    #[test]
    fn a_work_token_reaches_only_its_work_session_and_mounts() {
        let claims =
            WorkClaims { work: "work_1".into(), session: "sesn_1".into(), ws: "w".into(), org: "o".into() };
        let (get, post, del) = (Method::GET, Method::POST, Method::DELETE);
        assert!(work_route(&post, "/v1/environments/env_1/work/work_1/heartbeat", &claims));
        assert!(work_route(&get, "/v1/environments/env_1/work/work_1", &claims));
        assert!(!work_route(&get, "/v1/environments/env_1/work/work_2", &claims));
        assert!(!work_route(&get, "/v1/environments/env_1/work/poll", &claims));
        assert!(work_route(&get, "/v1/sessions/sesn_1", &claims));
        assert!(work_route(&get, "/v1/sessions/sesn_1/events/stream", &claims));
        assert!(work_route(&get, "/v1/sessions/sesn_1/events", &claims));
        assert!(work_route(&post, "/v1/sessions/sesn_1/events", &claims));
        assert!(!work_route(&post, "/v1/sessions/sesn_2/events", &claims));
        assert!(!work_route(&post, "/v1/sessions/sesn_1", &claims));
        assert!(!work_route(&post, "/v1/sessions/sesn_1/harness/events", &claims));
        assert!(!work_route(&del, "/v1/sessions/sesn_1", &claims));
        assert!(!work_route(&get, "/v1/sessions/sesn_2/events", &claims));
        assert!(work_route(&get, "/v1/files/file_1/content", &claims));
        assert!(work_route(&post, "/v1/files", &claims));
        assert!(work_route(&del, "/v1/files/file_1", &claims));
        assert!(!work_route(&get, "/v1/files", &claims));
        assert!(!work_route(&del, "/v1/files/file_1/content", &claims));
        assert!(work_route(&get, "/v1/skills/skill_1/versions/1", &claims));
        assert!(!work_route(&get, "/v1/agents", &claims));
        assert!(!work_route(&get, "/v1/sessions", &claims));
    }
}
