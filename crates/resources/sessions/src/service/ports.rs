//! The ports the sessions service is built over: what it needs from other
//! entities, and the session's work item.

use agents::AgentSnapshot;
use shared::ports::BoxFuture;
use shared::{ApiError, AuthContext, WorkspaceId};
use std::future::Future;

use crate::types::{Session, SessionId};

/// Resolution + validation dependencies the sessions service needs from
/// other entities — a port, so tests fake it and `api` wires it to
/// the real services (keeps the crate graph acyclic).
pub trait SessionDeps: Send + Sync {
    /// Resolve an agent reference (id + optional pinned version) to a frozen
    /// snapshot. Errors if missing or archived.
    fn resolve_agent<'a>(
        &'a self,
        ctx: &'a AuthContext,
        agent_id: &'a str,
        version: Option<u64>,
    ) -> BoxFuture<'a, Result<AgentSnapshot, ApiError>>;
    /// Refuse vaults the session may not use: missing from the workspace or
    /// archived.
    fn check_vaults<'a>(
        &'a self,
        ctx: &'a AuthContext,
        vault_ids: &'a [String],
    ) -> BoxFuture<'a, Result<(), ApiError>>;
    /// The environment's `config`, after checking it exists (org-scoped) and
    /// is not archived.
    fn resolve_environment<'a>(
        &'a self,
        ctx: &'a AuthContext,
        env_id: &'a str,
    ) -> BoxFuture<'a, Result<serde_json::Value, ApiError>>;
}

/// What an adapter implements to provide [`SessionDeps`]; the methods are documented there.
pub trait NativeSessionDeps: Send + Sync {
    fn resolve_agent(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> impl Future<Output = Result<AgentSnapshot, ApiError>> + Send;
    fn check_vaults(
        &self,
        ctx: &AuthContext,
        vault_ids: &[String],
    ) -> impl Future<Output = Result<(), ApiError>> + Send;
    fn resolve_environment(
        &self,
        ctx: &AuthContext,
        env_id: &str,
    ) -> impl Future<Output = Result<serde_json::Value, ApiError>> + Send;
}

impl<T: NativeSessionDeps> SessionDeps for T {
    fn resolve_agent<'a>(
        &'a self,
        ctx: &'a AuthContext,
        agent_id: &'a str,
        version: Option<u64>,
    ) -> BoxFuture<'a, Result<AgentSnapshot, ApiError>> {
        Box::pin(NativeSessionDeps::resolve_agent(self, ctx, agent_id, version))
    }
    fn check_vaults<'a>(
        &'a self,
        ctx: &'a AuthContext,
        vault_ids: &'a [String],
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(NativeSessionDeps::check_vaults(self, ctx, vault_ids))
    }
    fn resolve_environment<'a>(
        &'a self,
        ctx: &'a AuthContext,
        env_id: &'a str,
    ) -> BoxFuture<'a, Result<serde_json::Value, ApiError>> {
        Box::pin(NativeSessionDeps::resolve_environment(self, ctx, env_id))
    }
}

/// Queues and stops a session's work item.
pub trait SessionWork: Send + Sync {
    /// Put the session's work on its queue: its environment's for
    /// `self_hosted`, the runtime queue for `cloud`.
    fn queue<'a>(
        &'a self,
        ctx: &'a AuthContext,
        session: &'a Session,
        environment: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<(), ApiError>>;
    /// The session is archived or its create failed: its work item stops.
    fn end<'a>(&'a self, ws: &'a WorkspaceId, session: &'a SessionId) -> BoxFuture<'a, ()>;
}

/// What an adapter implements to provide [`SessionWork`]; the methods are documented there.
pub trait NativeSessionWork: Send + Sync {
    fn queue(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> impl Future<Output = Result<(), ApiError>> + Send;
    fn end(&self, ws: &WorkspaceId, session: &SessionId) -> impl Future<Output = ()> + Send;
}

impl<T: NativeSessionWork> SessionWork for T {
    fn queue<'a>(
        &'a self,
        ctx: &'a AuthContext,
        session: &'a Session,
        environment: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<(), ApiError>> {
        Box::pin(NativeSessionWork::queue(self, ctx, session, environment))
    }
    fn end<'a>(&'a self, ws: &'a WorkspaceId, session: &'a SessionId) -> BoxFuture<'a, ()> {
        Box::pin(NativeSessionWork::end(self, ws, session))
    }
}
