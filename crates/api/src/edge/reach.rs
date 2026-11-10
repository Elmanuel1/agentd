//! What each scoped credential reaches. A session's work token reaches its
//! own work item and session, the files, skills and memory stores its
//! session grants, and the files routes it saves outputs through. An
//! environment key reaches its queue's work and the sessions a worker serves,
//! which the sessions routes narrow to its environment. Every other route
//! answers as live does for each.

use async_trait::async_trait;
use axum::http::Method;
use shared::{ApiError, AuthContext, ErrorType};
use work::WorkClaims;

/// Live's answer to an environment key on a route it does not reach.
const SCOPE: &str = "OAuth token does not meet scope requirement any_of(org:sessions:claude_code, \
                     user:developer, user:sessions:claude_code, workspace:developer)";

/// 403 `permission_error` with `message`.
pub(super) fn permission(message: impl Into<String>) -> ApiError {
    ApiError {
        status: ErrorType::Permission.status(),
        error_type: ErrorType::Permission,
        message: message.into(),
        extra: serde_json::Map::new(),
    }
}

/// Live's 403 for an environment key off its routes.
pub(super) fn out_of_scope() -> ApiError {
    permission(SCOPE)
}

fn parts(path: &str) -> Vec<&str> {
    path.trim_start_matches('/').split('/').collect()
}

/// The routes a work token reaches: its own work item's read, ack,
/// heartbeat and stop; its own session's reads, events and stream, and the
/// harness routes; the files, skills and memory routes the official worker
/// calls, which [`granted`] narrows to what its session grants.
pub(super) fn work_route(method: &Method, path: &str, claims: &WorkClaims) -> bool {
    let own = |id: &str| id == claims.session;
    let (get, post, delete) = (method == Method::GET, method == Method::POST, method == Method::DELETE);
    match parts(path).as_slice() {
        ["v1", "environments", _, "work", id] => get && own(id),
        ["v1", "environments", _, "work", id, "ack" | "heartbeat" | "stop"] => post && own(id),
        ["v1", "sessions", s] | ["v1", "sessions", s, "events", "stream"] => get && own(s),
        ["v1", "sessions", s, "events"] => (get || post) && own(s),
        ["v1", "sessions", s, "harness", ..] => own(s),
        ["v1", "files"] => post,
        ["v1", "files", _] => get || delete,
        ["v1", "files", _, "content"] | ["v1", "skills", _, "versions", _] => get,
        ["v1", "skills", _, "versions", _, "content"] => get,
        ["v1", "memory_stores", _, "memories"] => get || post,
        ["v1", "memory_stores", _, "memories", _] => get || post || delete,
        _ => false,
    }
}

/// The routes an environment key reaches on `queue`, as live serves a
/// worker's key: its queue's work list, poll, stats, ack, heartbeat and stop,
/// and session reads and events. Reading the environment or one work item
/// is not among them (live: 403).
pub(super) fn environment_route(method: &Method, path: &str, queue: &str) -> bool {
    let (get, post) = (method == Method::GET, method == Method::POST);
    match parts(path).as_slice() {
        ["v1", "environments", env, "work"] | ["v1", "environments", env, "work", "poll" | "stats"] => {
            get && *env == queue
        }
        ["v1", "environments", env, "work", _, "ack" | "heartbeat" | "stop"] => post && *env == queue,
        ["v1", "sessions", _] | ["v1", "sessions", _, "events", "stream"] => get,
        ["v1", "sessions", _, "events"] => get || post,
        _ => false,
    }
}

/// The session a path names: a work item's, or a session route's.
pub(super) fn named_session(path: &str) -> Option<&str> {
    match parts(path).as_slice() {
        ["v1", "environments", _, "work", "poll" | "stats"] => None,
        ["v1", "environments", _, "work", id, ..] | ["v1", "sessions", id, ..] => Some(id),
        _ => None,
    }
}

/// The environment `GET /v1/environments/{id}` reads.
pub(super) fn environment_read<'a>(method: &Method, path: &'a str) -> Option<&'a str> {
    match parts(path).as_slice() {
        ["v1", "environments", id] if method == Method::GET => Some(id),
        _ => None,
    }
}

/// `GET /v1/environments/{id}/work/poll`.
pub(super) fn is_poll(path: &str) -> bool {
    matches!(parts(path).as_slice(), ["v1", "environments", _, "work", "poll"])
}

/// What a session grants its work token beyond the session itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grants {
    /// Its agent's skills.
    pub skills: Vec<String>,
    /// The memory stores it attaches, each with whether it may write them.
    pub memory_stores: Vec<(String, bool)>,
}

/// What the edge asks of sessions for a work token.
#[async_trait]
pub trait SessionGrants: Send + Sync {
    /// What `session` grants its work token.
    async fn grants(&self, ctx: &AuthContext, session: &str) -> Result<Grants, ApiError>;
}

/// Whether `session`'s grants cover a skill or memory route; other routes
/// need nothing from the session. A skill or store it does not grant does not
/// exist for its token; a write to a store it attaches `read_only` is a 403.
pub(super) async fn granted(
    method: &Method,
    path: &str,
    ctx: &AuthContext,
    session: &str,
    grants: &dyn SessionGrants,
) -> Result<(), ApiError> {
    match parts(path).as_slice() {
        ["v1", "skills", id, ..] => {
            if !grants.grants(ctx, session).await?.skills.iter().any(|s| s == id) {
                return Err(ApiError::not_found(format!("Skill not found: {id}")));
            }
            Ok(())
        }
        ["v1", "memory_stores", store, ..] => {
            let found =
                grants.grants(ctx, session).await?.memory_stores.into_iter().find(|(s, _)| s == store);
            match found {
                None => Err(ApiError::not_found("memory store not found")),
                Some((_, false)) if method != Method::GET => {
                    Err(permission(format!("memory store {store} is attached to this session read_only")))
                }
                Some(_) => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
