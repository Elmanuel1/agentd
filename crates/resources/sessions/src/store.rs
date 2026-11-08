//! Session persistence: session rows ([`SessionsStore`]) and the append-only
//! event log ([`SessionEventLog`]).

use async_trait::async_trait;
use shared::page::filters::CreatedRange;
use shared::{Pagination, Timestamp, WorkspaceId};

use crate::events::Event;
use crate::types::{Session, SessionId, SessionStatus, ThreadId};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such session.
    #[error("session not found")]
    NotFound,
    /// The session changed after the caller read it.
    #[error("session changed")]
    Conflict,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// List order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Order {
    /// Oldest first.
    Asc,
    /// Newest first.
    #[default]
    Desc,
}

/// The sessions list's filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionFilter {
    /// Only this agent's sessions.
    pub agent_id: Option<String>,
    /// Only sessions on this agent version.
    pub agent_version: Option<u64>,
    /// Creation-time bounds.
    pub created: CreatedRange,
    /// Only sessions a deployment created.
    pub deployment_id: Option<String>,
    /// Keep archived sessions.
    pub include_archived: bool,
    /// Only sessions that mount this memory store.
    pub memory_store_id: Option<String>,
    /// Newest first unless `asc`.
    pub order: Order,
    /// Only sessions in these statuses (any status when empty).
    pub statuses: Vec<SessionStatus>,
}

/// The events list's filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilter {
    /// Bounds on `processed_at`.
    pub processed: CreatedRange,
    /// Oldest first unless `desc`.
    pub order: Order,
    /// Only these event types (any type when empty).
    pub types: Vec<String>,
    /// `None`: the session's own events (see `Event::on_session_stream`); a
    /// secondary thread's id: that thread's events.
    pub thread: Option<ThreadId>,
}

impl Default for EventFilter {
    fn default() -> Self {
        Self { processed: CreatedRange::default(), order: Order::Asc, types: Vec::new(), thread: None }
    }
}

/// Persistence port.
#[async_trait]
pub trait SessionsStore: Send + Sync {
    /// Insert a session.
    async fn insert(&self, ws: &WorkspaceId, s: Session) -> Result<(), StoreError>;
    /// Fetch a session.
    async fn get(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError>;
    /// Write `s` only while the stored session is still `read`; a session
    /// another writer changed first is [`StoreError::Conflict`].
    async fn replace(&self, ws: &WorkspaceId, read: &Session, s: Session) -> Result<(), StoreError>;
    /// Delete a session row.
    async fn delete(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError>;
    /// One page of the sessions `filter` keeps, in its order.
    async fn list(
        &self,
        ws: &WorkspaceId,
        filter: &SessionFilter,
        page: &Pagination,
    ) -> Result<Vec<Session>, StoreError>;
}

/// A session's append-only event log. It has its own lifecycle: the engine
/// appends to it while the session row is unchanged, and the events list and
/// stream read it on its own.
#[async_trait]
pub trait SessionEventLog: Send + Sync {
    /// Append an event to the log of an existing session.
    async fn append_event(&self, ws: &WorkspaceId, id: &SessionId, event: Event) -> Result<(), StoreError>;
    /// Full event history, append order.
    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError>;
    /// Stamp the client events the agent has not taken yet processed at `at`,
    /// in one statement; those events as stamped, append order.
    async fn update_processed_at(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        at: Timestamp,
    ) -> Result<Vec<Event>, StoreError>;
    /// One page of the events `filter` keeps, in its order.
    async fn events_page(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        filter: &EventFilter,
        page: &Pagination,
    ) -> Result<Vec<Event>, StoreError>;
    /// Delete the whole log.
    async fn delete_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<(), StoreError>;
}
