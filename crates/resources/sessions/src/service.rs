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
use shared::limits::ModelCatalog;
use shared::ports::Runtime;
use shared::{ApiError, AuthContext, Usage, WorkspaceId};
use tokio::sync::broadcast;

use crate::events::{Event, EventPayload, StopReason};
use crate::store::{SessionEventLog, SessionsStore, StoreError};
use crate::types::{OutcomeEvaluation, Session, SessionId, SessionStatus, ThreadId};

use agents::AgentSnapshot;

mod engine;
mod events;
mod lifecycle;
mod resources;
mod sandbox;
mod threads;

pub use threads::threads_projection;

pub use events::Subscription;

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

/// What the engine (the agent loop in agentd) may do to a session: the
/// sessions to engine seam.
#[async_trait]
pub trait EngineContract: Send + Sync {
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
    /// Whether client events wait to be taken, without taking them.
    async fn has_unprocessed(&self, ws: &WorkspaceId, session: &SessionId) -> Result<bool, ApiError>;
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

/// Starts the agent loop when a session has new client events. Waking a
/// session whose loop already runs only nudges that loop.
pub trait TurnTrigger: Send + Sync {
    /// The session has unprocessed client events; `ctx` is the caller who
    /// sent them, whose workspace and organization the loop acts in.
    fn wake(&self, ctx: &AuthContext, session: &SessionId);
}

/// The live fan-out: one broadcast channel per session that has a listener.
type Channels = Arc<Mutex<HashMap<SessionId, broadcast::Sender<Event>>>>;

/// Sessions service.
pub struct SessionsService {
    store: Arc<dyn SessionsStore>,
    events: Arc<dyn SessionEventLog>,
    deps: Arc<dyn SessionDeps>,
    /// Live SSE fan-out. Stream has NO replay — reconnect = list + dedupe.
    channels: Channels,
    /// The models an `agent_with_overrides` may name.
    catalog: ModelCatalog,
    /// Runs one sandbox per session; `None` runs no sandbox.
    sandbox: Option<Arc<dyn Runtime>>,
    /// Runs the agent loop; unset, the service only stores events.
    trigger: std::sync::OnceLock<Arc<dyn TurnTrigger>>,
}

impl std::fmt::Debug for SessionsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionsService").finish_non_exhaustive()
    }
}

impl SessionsService {
    /// Build with the session rows, their event log, the dependency port and
    /// the model catalog.
    pub fn new(
        store: Arc<dyn SessionsStore>,
        events: Arc<dyn SessionEventLog>,
        deps: Arc<dyn SessionDeps>,
        catalog: ModelCatalog,
    ) -> Self {
        Self {
            store,
            events,
            deps,
            catalog,
            channels: Channels::default(),
            sandbox: None,
            trigger: std::sync::OnceLock::new(),
        }
    }

    /// Run each session's sandbox on `runtime`.
    #[must_use]
    pub fn with_sandbox(mut self, runtime: Arc<dyn Runtime>) -> Self {
        self.sandbox = Some(runtime);
        self
    }

    /// Run the agent loop through `trigger` when client events arrive. Set
    /// once, after construction, because the loop calls back into this
    /// service.
    pub fn set_trigger(&self, trigger: Arc<dyn TurnTrigger>) {
        let _ = self.trigger.set(trigger);
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
        StoreError::Conflict => {
            ApiError::conflict_invalid_request("the session changed while this request ran; retry it")
        }
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
