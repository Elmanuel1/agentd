//! Session persistence: session rows ([`SessionsStore`]) and the append-only
//! event log ([`SessionEventLog`], read through [`SessionEventReads`]).

use shared::page::filters::CreatedRange;
use shared::ports::BoxFuture;
use shared::{ApiError, Pagination, Timestamp, WorkspaceId};
use std::future::Future;

use crate::events::{Event, EventId};
use crate::types::{Session, SessionId, SessionStatus, ThreadId};

mod reads;

pub use reads::{Head, NativeSessionEventReads, SessionEventReads, Tail};

mod native;
pub use native::{NativeSessionEventLog, NativeSessionReplace, NativeSessionsStore};

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

/// A session's compare-and-write.
pub trait SessionReplace: Send + Sync {
    /// Write `s` and `deliveries` only while the stored session is still `read`, else `Conflict`.
    fn replace<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        read: &'a Session,
        s: Session,
        deliveries: &'a [webhooks::Delivery],
    ) -> BoxFuture<'a, Result<(), StoreError>>;
}

/// Persistence port.
pub trait SessionsStore: SessionReplace {
    /// Insert a session.
    fn insert<'a>(&'a self, ws: &'a WorkspaceId, s: Session) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Fetch a session.
    fn get<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Session, StoreError>>;
    /// Under the session's append lock, delete the session unless it is
    /// running ([`StoreError::Conflict`]) and stop its work item.
    fn delete<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Session, StoreError>>;
    /// One page of the sessions `filter` keeps, in its order.
    fn list<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        filter: &'a SessionFilter,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Session>, StoreError>>;
}

/// Whether an append puts its session on the step queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// It does not: an agent's own event.
    No,
    /// It does, in the append's commit.
    Step,
}

/// Refuses results, given the calls they answer and the results already saved;
/// accepted, it answers each result's thread, its call's.
pub type AnswerCheck<'a> = dyn Fn(&[Event]) -> Result<Vec<Option<ThreadId>>, ApiError> + Send + Sync + 'a;

/// A session's append-only event log. It has its own lifecycle: the engine
/// appends to it while the session row is unchanged, and the events list and
/// stream read it on its own.
pub trait SessionEventLog: SessionEventReads {
    /// Append `events`, in order, to the log of an existing session, under
    /// the session's append lock.
    fn append_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        events: &'a [Event],
        wake: Wake,
    ) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Stamp `at` on those of `ids` not processed yet; the events it stamped,
    /// in log order.
    fn update_processed_at<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        ids: &'a [EventId],
        at: Timestamp,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>>;
    /// Under the session's append lock: `check` reads the events whose id is
    /// one of `calls` and the results that answer them, and `events` are
    /// appended only when it passes; otherwise its refusal is returned.
    fn append_answers<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        calls: &'a [EventId],
        events: &'a [Event],
        check: &'a AnswerCheck<'a>,
        wake: Wake,
    ) -> BoxFuture<'a, Result<Result<(), ApiError>, StoreError>>;
    /// Delete the whole log of each session.
    fn delete_events_of<'a>(
        &'a self,
        sessions: &'a [(WorkspaceId, SessionId)],
    ) -> BoxFuture<'a, Result<(), StoreError>>;
}

/// How loaded the database is, as a replica last measured it.
pub trait DatabaseLoad: Send + Sync {
    /// Too loaded to take new streams.
    fn overloaded(&self) -> bool;
}
