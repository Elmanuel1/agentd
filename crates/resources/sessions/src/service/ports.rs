//! The ports the sessions service is built over: what it needs from other
//! entities, and the session's work item.

use agents::AgentSnapshot;
use async_trait::async_trait;
use shared::{ApiError, AuthContext, WorkspaceId};

use crate::types::{Session, SessionId};

/// Resolution + validation dependencies the sessions service needs from
/// other entities — a port, so tests fake it and `api` wires it to
/// the real services (keeps the crate graph acyclic).
#[async_trait]
pub trait SessionDeps: Send + Sync {
    /// Resolve an agent reference (id + optional pinned version) to a frozen
    /// snapshot. Errors if missing or archived.
    async fn resolve_agent(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentSnapshot, ApiError>;
    /// The environment's `config`, after checking it exists (org-scoped) and
    /// is not archived.
    async fn resolve_environment(
        &self,
        ctx: &AuthContext,
        env_id: &str,
    ) -> Result<serde_json::Value, ApiError>;
}

/// Queues and stops a session's work item.
#[async_trait]
pub trait SessionWork: Send + Sync {
    /// Put the session's work on its queue: its environment's for
    /// `self_hosted`, the runtime queue for `cloud`.
    async fn queue(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<(), ApiError>;
    /// The session is archived or its create failed: its work item stops.
    async fn end(&self, ws: &WorkspaceId, session: &SessionId);
}
