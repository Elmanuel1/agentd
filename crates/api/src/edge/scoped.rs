//! The scoped credentials: environment keys (a self-hosted environment's,
//! and agentd's runtime key) and sessions' work tokens. What each reaches is
//! in [`super::reach`].

use std::sync::Arc;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use shared::ports::keys::{EnvironmentGrant, EnvironmentKeySource};
use shared::{wire_response, ApiError, AuthContext, OrgId, SessionCaller, WorkspaceId};
use work::store::Reach;
use work::{WorkCaller, WorkReads, WorkState, WorkTokens, RUNTIME};

use super::auth::{run, run_in_workspace};
use super::reach::{
    environment_read, environment_route, granted, named_session, out_of_scope, work_route, SessionGrants,
};

/// The runtime key's actor, as every write it makes records.
const RUNTIME_ACTOR: &str = "runtime";

/// What work tokens need: their signer, their items, and what their sessions grant.
#[derive(Clone)]
pub struct WorkAccess {
    /// Signs and checks work tokens.
    pub tokens: WorkTokens,
    /// The work items, read per token request.
    pub items: Arc<dyn WorkReads>,
    /// What each session grants its token.
    pub grants: Arc<dyn SessionGrants>,
}

impl std::fmt::Debug for WorkAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkAccess")
    }
}

/// An environment key. Live: it is a Bearer token only, and a route its
/// worker does not use is a 403.
pub(super) async fn environment_key(
    keys: &dyn EnvironmentKeySource,
    work: &WorkAccess,
    req: Request,
    next: Next,
    key: &str,
    bearer: bool,
) -> Response {
    if !bearer {
        return wire_response(&ApiError::authentication("invalid x-api-key"));
    }
    match keys.resolve(key).await {
        Err(e) => wire_response(&ApiError::internal(e.to_string())),
        Ok(None) => wire_response(&ApiError::authentication("Invalid bearer token")),
        Ok(Some(EnvironmentGrant::Environment(ctx, caller))) => {
            if !environment_route(req.method(), req.uri().path(), &caller.queue) {
                return wire_response(&out_of_scope());
            }
            let mut req = req;
            let actor = ctx.api_key_id.clone();
            let queue = caller.queue.clone();
            req.extensions_mut().insert(caller);
            req.extensions_mut().insert(WorkCaller::Queue { queue, ctx: Some(ctx.clone()) });
            run_in_workspace(ctx, actor, req, next).await
        }
        Ok(Some(EnvironmentGrant::Runtime)) => runtime_key(work.items.as_ref(), req, next).await,
    }
}

fn runtime_ctx(org: OrgId, workspace: WorkspaceId) -> AuthContext {
    AuthContext { org, workspace, api_key_id: RUNTIME_ACTOR.to_owned() }
}

/// agentd's runtime key: the runtime queue's routes, whose items span
/// workspaces, and the environment of a session on the runtime queue (D39).
/// A route naming a session runs in that session's workspace.
async fn runtime_key(items: &dyn WorkReads, mut req: Request, next: Next) -> Response {
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    if let Some(environment) = environment_read(&method, &path) {
        return match items.runtime_owner(environment).await {
            Ok(Some((org, ws))) => {
                run_in_workspace(runtime_ctx(org, ws), RUNTIME_ACTOR.into(), req, next).await
            }
            Ok(None) => wire_response(&out_of_scope()),
            Err(e) => wire_response(&ApiError::internal(e.to_string())),
        };
    }
    if !environment_route(&method, &path, RUNTIME) {
        return wire_response(&out_of_scope());
    }
    let Some(session) = named_session(&path) else {
        req.extensions_mut().insert(WorkCaller::Queue { queue: RUNTIME.into(), ctx: None });
        return run(RUNTIME_ACTOR.into(), req, next).await;
    };
    let item = match items.find_on_queue(RUNTIME, session).await {
        Ok(item) => item,
        Err(e) => return wire_response(&ApiError::internal(e.to_string())),
    };
    let Some(item) = item else {
        let what = match path.starts_with("/v1/sessions/") {
            true => "session not found",
            false => "work not found",
        };
        return wire_response(&ApiError::not_found(what));
    };
    let ctx = runtime_ctx(item.org, item.workspace);
    req.extensions_mut().insert(WorkCaller::Queue { queue: RUNTIME.into(), ctx: Some(ctx.clone()) });
    run_in_workspace(ctx, RUNTIME_ACTOR.into(), req, next).await
}

/// A session's work token: valid while its item is not `stopped`, on its
/// routes only, reaching what its session grants.
pub(super) async fn work_token(work: &WorkAccess, mut req: Request, next: Next, key: &str) -> Response {
    let invalid = || wire_response(&ApiError::authentication("invalid work token"));
    let Some(claims) = work.tokens.verify(key).filter(|c| work_route(req.method(), req.uri().path(), c))
    else {
        return invalid();
    };
    let ws = WorkspaceId::new(claims.ws.clone());
    match work.items.get(&ws, &claims.session, &Reach::default()).await {
        Ok(Some(item)) if item.work.state != WorkState::Stopped => {}
        Ok(_) => return invalid(),
        Err(e) => return wire_response(&ApiError::internal(e.to_string())),
    }
    let ctx = AuthContext {
        org: OrgId::new(claims.org.clone()),
        workspace: ws,
        api_key_id: claims.session.clone(),
    };
    if let Err(e) = granted(req.method(), req.uri().path(), &ctx, &claims.session, work.grants.as_ref()).await
    {
        return wire_response(&e);
    }
    let session = claims.session.clone();
    req.extensions_mut().insert(SessionCaller { session: session.clone() });
    req.extensions_mut().insert(WorkCaller::Token(claims));
    run_in_workspace(ctx, session, req, next).await
}
