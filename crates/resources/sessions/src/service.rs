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
use std::sync::Arc;

use shared::limits::ModelCatalog;
use shared::ports::Runtime;
use shared::ApiError;

use crate::store::{SessionEventLog, SessionEventReads, SessionsStore, StoreError};
use crate::types::{Session, SessionId};

mod events;
mod harness;
mod lifecycle;
mod ports;
mod record;
mod resources;
mod sandbox;
mod stream;
mod threads;
mod waiting;

pub use threads::threads_projection;

pub use ports::{HarnessContract, SandboxWork, SessionDeps};
pub use stream::{Streams, Subscription};

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
