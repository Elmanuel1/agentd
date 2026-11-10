//! Who calls the work routes, and which items each reaches: a workspace key
//! its workspace's environments, an environment key its queue, a work token
//! its own item.

use shared::{ApiError, AuthContext, WorkspaceId};

use crate::service::{not_found, WorkService};
use crate::store::{Reach, Scope, RUNTIME};
use crate::tokens::WorkClaims;

/// Who calls the work routes, as the edge resolved them.
#[derive(Debug, Clone)]
pub enum WorkCaller {
    /// A workspace API key: its workspace's environments.
    Workspace(AuthContext),
    /// An environment key: one queue. `ctx` is the key's workspace, or for
    /// the runtime key the workspace of the session its path names.
    Queue {
        /// The queue.
        queue: String,
        /// The workspace, when known.
        ctx: Option<AuthContext>,
    },
    /// A session's work token: its own item only.
    Token(WorkClaims),
}

impl WorkService {
    /// The workspace and reach of item `id` at environment `env` for `caller`.
    pub(crate) fn target(
        &self,
        caller: &WorkCaller,
        env: &str,
        id: &str,
    ) -> Result<(WorkspaceId, Reach), ApiError> {
        match caller {
            WorkCaller::Workspace(ctx) => {
                Ok((ctx.workspace.clone(), Reach { environment: Some(env.to_owned()), queue: None }))
            }
            WorkCaller::Queue { queue, ctx: Some(ctx) } => {
                Ok((ctx.workspace.clone(), Reach { environment: None, queue: Some(queue.clone()) }))
            }
            WorkCaller::Queue { ctx: None, .. } => Err(not_found()),
            WorkCaller::Token(claims) if claims.session == id => Ok((
                WorkspaceId::new(claims.ws.clone()),
                Reach { environment: Some(env.to_owned()), queue: Some(env.to_owned()) },
            )),
            WorkCaller::Token(_) => Err(not_found()),
        }
    }
}

/// The items a list or stats call by `caller` at `env` covers. A work
/// token reaches its own item only, never the environment's.
pub(crate) fn scope(caller: &WorkCaller, env: &str) -> Result<Scope, ApiError> {
    match caller {
        WorkCaller::Workspace(ctx) => Ok(Scope::Environment {
            workspace: ctx.workspace.clone(),
            environment: env.to_owned(),
            queue: None,
        }),
        WorkCaller::Queue { queue, .. } if queue == RUNTIME => Ok(Scope::Queue(queue.clone())),
        WorkCaller::Queue { queue, ctx: Some(ctx) } => Ok(Scope::Environment {
            workspace: ctx.workspace.clone(),
            environment: queue.clone(),
            queue: Some(queue.clone()),
        }),
        WorkCaller::Queue { ctx: None, .. } | WorkCaller::Token(_) => Err(not_found()),
    }
}
