//! The ports the sessions service is built over: what it needs from other
//! entities, what a session's harness writes, and the sandbox's work item.

use std::collections::BTreeMap;

use agents::AgentSnapshot;
use async_trait::async_trait;
use shared::{ApiError, AuthContext, Usage, WorkspaceId};

use crate::events::{Event, EventPayload, StopReason};
use crate::types::{OutcomeEvaluation, Session, SessionId, SessionStatus, ThreadId};

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

/// What a session's harness, the agent loop in its sandbox, writes to the
/// session: its events, status, usage and outcome evaluations.
#[async_trait]
pub trait HarnessContract: Send + Sync {
    /// The session.
    async fn session(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Session, ApiError>;
    /// Every event of the session and its threads, oldest first.
    async fn events(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Vec<Event>, ApiError>;
    /// Append an agent, span or session event and broadcast it; `thread`
    /// names a secondary thread or the thread a lifecycle event is about.
    async fn emit(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        thread: Option<&ThreadId>,
        payload: EventPayload,
    ) -> Result<Event, ApiError>;
    /// Transition status (emits the matching `session.status_*` event).
    async fn set_status(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        status: SessionStatus,
        stop_reason: Option<StopReason>,
    ) -> Result<(), ApiError>;
    /// Pull queued (unprocessed) user events, stamping them processed.
    async fn drain_user_events(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Vec<Event>, ApiError>;
    /// Accumulate usage onto the session.
    async fn add_usage(&self, ws: &WorkspaceId, session: &SessionId, usage: &Usage) -> Result<(), ApiError>;
    /// Record an outcome evaluation onto the session object.
    async fn record_outcome(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        eval: OutcomeEvaluation,
    ) -> Result<(), ApiError>;
}

/// Issues the work item a session's sandbox runs, and registers the session
/// with the egress service.
#[async_trait]
pub trait SandboxWork: Send + Sync {
    /// The environment variables the sandbox starts with: where its harness
    /// reports, the work it runs, and its egress settings.
    async fn issue(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<BTreeMap<String, String>, ApiError>;
    /// The session's sandbox is gone: its work item stops.
    async fn end(&self, ws: &WorkspaceId, session: &SessionId);
    /// The session is gone and its work item already stopped: only what is
    /// held for its sandbox goes.
    fn forget(&self, session: &SessionId);
}
