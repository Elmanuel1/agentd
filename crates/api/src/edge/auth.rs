//! Authentication: the API key from `x-api-key` or `Authorization: Bearer`.
//! The route decides which store is asked: `/v1/organizations` routes take
//! admin keys only, every other route workspace keys, work tokens and
//! environment keys, told apart by prefix ([`super::scoped`]). What a scoped
//! credential reaches is in [`super::reach`].

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use shared::ports::keys::material::KeyKind;
use shared::ports::keys::{AdminKeySource, EnvironmentKeySource, KeySource};
use shared::{current_request_id, wire_response, with_writer, ApiError, AuthContext, Writer};
use work::WorkCaller;

use super::reach::is_poll;
use super::scoped::{environment_key, work_token, WorkAccess};

/// What a request may authenticate with: a workspace key, an admin key, an
/// environment key, or a session's work token.
#[derive(Clone)]
pub struct Credentials {
    /// Workspace keys.
    pub keys: Arc<dyn KeySource>,
    /// Admin keys.
    pub admin_keys: Arc<dyn AdminKeySource>,
    /// Environment keys, agentd's runtime key among them.
    pub environment_keys: Arc<dyn EnvironmentKeySource>,
    /// Work tokens and what they reach.
    pub work: Option<WorkAccess>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials").field("work", &self.work.is_some()).finish_non_exhaustive()
    }
}

/// The caller's key, and whether it came as `Authorization: Bearer`.
struct Presented {
    key: String,
    bearer: bool,
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
    let Some(Presented { key, bearer }) = api_key(req.headers()) else {
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
        return with_tenant(run(actor, req, next).await, tenant);
    }
    if key.starts_with(KeyKind::Environment.prefix()) {
        return environment_key(
            creds.environment_keys.as_ref(),
            creds.work.as_ref(),
            req,
            next,
            &key,
            bearer,
        )
        .await;
    }
    if key.starts_with(work::tokens::PREFIX) {
        return work_token(creds.work.as_ref(), req, next, &key).await;
    }
    let ctx = match creds.keys.resolve(&key).await {
        Ok(Some(ctx)) => ctx,
        Ok(None) => return wire_response(&ApiError::authentication("invalid x-api-key")),
        Err(e) => return wire_response(&ApiError::internal(e.to_string())),
    };
    // Live: polling takes an environment key; a workspace key is no bearer token there.
    if is_poll(req.uri().path()) {
        return wire_response(&ApiError::authentication("Invalid bearer token"));
    }
    let actor = ctx.api_key_id.clone();
    req.extensions_mut().insert(WorkCaller::Workspace(ctx.clone()));
    run_in_workspace(ctx, actor, req, next).await
}

/// Run the request in `ctx`'s workspace as `actor`, naming the tenant on the response.
pub(super) async fn run_in_workspace(
    ctx: AuthContext,
    actor: String,
    mut req: Request,
    next: Next,
) -> Response {
    let tenant = [
        ("anthropic-organization-id", HeaderValue::from_str(ctx.org.as_str()).ok()),
        ("anthropic-workspace-id", HeaderValue::from_str(ctx.workspace.as_str()).ok()),
    ];
    req.extensions_mut().insert::<AuthContext>(ctx);
    with_tenant(run(actor, req, next).await, tenant)
}

/// Run the request as `actor`.
pub(super) async fn run(actor: String, req: Request, next: Next) -> Response {
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

fn api_key(headers: &HeaderMap) -> Option<Presented> {
    if let Some(key) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(Presented { key: key.to_owned(), bearer: false });
    }
    let auth = headers.get("authorization")?.to_str().ok()?;
    let (scheme, token) = auth.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| Presented { key: token.trim().to_owned(), bearer: true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_comes_from_x_api_key_or_a_bearer_token() {
        let mut h = HeaderMap::new();
        h.insert("authorization", HeaderValue::from_static("Bearer sk-1"));
        assert!(api_key(&h).is_some_and(|p| p.key == "sk-1" && p.bearer));
        h.insert("x-api-key", HeaderValue::from_static("sk-2"));
        assert!(api_key(&h).is_some_and(|p| p.key == "sk-2" && !p.bearer));
        let mut basic = HeaderMap::new();
        basic.insert("authorization", HeaderValue::from_static("Basic abc"));
        assert!(api_key(&basic).is_none());
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
}
