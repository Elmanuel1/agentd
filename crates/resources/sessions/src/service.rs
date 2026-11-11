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
use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use shared::limits::ModelCatalog;
use shared::ports::Runtime;
use shared::{ApiError, AuthContext, Usage, WorkspaceId};

use crate::events::{Event, EventPayload, StopReason};
use crate::store::{SessionEventLog, SessionEventReads, SessionsStore, StoreError};
use crate::types::{OutcomeEvaluation, Session, SessionId, SessionStatus, ThreadId};

use agents::AgentSnapshot;

mod events;
mod harness;
mod lifecycle;
mod record;
mod resources;
mod sandbox;
mod stream;
mod threads;
mod waiting;

pub use threads::threads_projection;

pub use stream::{Streams, Subscription};

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

/// Sessions service.
pub struct SessionsService {
    store: Arc<dyn SessionsStore>,
    events: Arc<dyn SessionEventLog>,
    /// The log as the events list reads it: a read replica, where one is set.
    list_reads: Arc<dyn SessionEventReads>,
    deps: Arc<dyn SessionDeps>,
    /// Live SSE fan-out. Stream has NO replay — reconnect = list + dedupe.
    streams: Streams,
    /// The models an `agent_with_overrides` may name.
    catalog: ModelCatalog,
    /// Runs one sandbox per session; `None` runs no sandbox.
    sandbox: Option<Arc<dyn Runtime>>,
    /// Issues each sandbox's work item, which its harness runs.
    work: Option<Arc<dyn SandboxWork>>,
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
        let list_reads = events.clone();
        Self {
            store,
            events,
            list_reads,
            deps,
            catalog,
            streams: Streams::default(),
            sandbox: None,
            work: None,
        }
    }

    /// Stream through `streams`, for every replica.
    #[must_use]
    pub fn with_streams(mut self, streams: Streams) -> Self {
        self.streams = streams;
        self
    }

    /// Serve the events list from `reads`.
    #[must_use]
    pub fn with_list_reads(mut self, reads: Arc<dyn SessionEventReads>) -> Self {
        self.list_reads = reads;
        self
    }

    /// Run each session's sandbox on `runtime`, its worker on a work item
    /// from `work`.
    #[must_use]
    pub fn with_sandbox(mut self, runtime: Arc<dyn Runtime>, work: Option<Arc<dyn SandboxWork>>) -> Self {
        self.sandbox = Some(runtime);
        self.work = work;
        self
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
