//! Session business logic: lifecycle, event ingest, the status state
//! machine, live streaming, and the engine contract.
//!
//! State machine (documented + probe-consistent):
//! ```text
//! rescheduling → running ↔ idle → terminated
//! ```
//! - events can be sent while `running` or `idle`; queued in order,
//! - `terminated` is absorbing; archived sessions are read-only,
//! - `session.status_idle` carries a `stop_reason`; `requires_action` is
//!   NOT terminal for drain loops.
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use shared::{ApiError, AuthContext, Usage, WorkspaceId};
use tokio::sync::broadcast;

use crate::events::{Event, EventPayload, StopReason};
use crate::store::{SessionEventLog, SessionsStore, StoreError};
use crate::types::{OutcomeEvaluation, Session, SessionId, SessionStatus};

use agents::AgentSnapshot;

mod events;
mod lifecycle;

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
    /// Verify the environment exists (org-scoped) and is not archived.
    async fn check_environment(&self, ctx: &AuthContext, env_id: &str) -> Result<(), ApiError>;
}

/// What the engine (task-side loop) is allowed to do to a session — the
/// sessions↔engine seam.
#[async_trait]
pub trait EngineContract: Send + Sync {
    /// Append an agent/span event and broadcast it.
    async fn emit(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
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

/// Sessions service.
pub struct SessionsService {
    store: Arc<dyn SessionsStore>,
    events: Arc<dyn SessionEventLog>,
    deps: Arc<dyn SessionDeps>,
    /// Live SSE fan-out. Stream has NO replay — reconnect = list + dedupe.
    channels: Mutex<HashMap<SessionId, broadcast::Sender<Event>>>,
}

impl std::fmt::Debug for SessionsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionsService").finish_non_exhaustive()
    }
}

impl SessionsService {
    /// Build with the session rows, their event log and the dependency port.
    pub fn new(
        store: Arc<dyn SessionsStore>,
        events: Arc<dyn SessionEventLog>,
        deps: Arc<dyn SessionDeps>,
    ) -> Self {
        Self { store, events, deps, channels: Mutex::new(HashMap::new()) }
    }
}

fn guard_writable(s: &Session) -> Result<(), ApiError> {
    if s.archived_at.is_some() {
        return Err(ApiError::conflict_invalid_request("session is archived and read-only"));
    }
    Ok(())
}

fn parse_id(raw: &str) -> Result<SessionId, ApiError> {
    SessionId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("session not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
