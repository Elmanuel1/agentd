//! A session's event log, as it is read.

use shared::ports::BoxFuture;
use shared::{Pagination, WorkspaceId};
use std::future::Future;

use super::{EventFilter, StoreError};
use crate::events::Event;
use crate::types::SessionId;

/// An event's place in its session's log: the transaction that wrote it,
/// then its number. Writers append without a lock, so a lower number can
/// commit later; a reader that pages by (`xid`, `seq`) below the oldest
/// running transaction never skips it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    /// The writing transaction's id.
    pub xid: i64,
    /// The event's number.
    pub seq: i64,
}

/// A read of a session's log from a position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tail {
    /// The session is archived.
    pub archived: bool,
    /// Each event with its position in the log, in log order.
    pub events: Vec<(Pos, Event)>,
}

/// Where a new stream of a session starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    /// The position of the session's latest event, the default when it has none.
    pub pos: Pos,
    /// The positions of its client events not processed yet, in log order.
    pub queued: Vec<i64>,
}

/// A session's event log, read.
pub trait SessionEventReads: Send + Sync {
    /// Full event history, append order.
    fn events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>>;
    /// The client events the agent has not taken yet, append order.
    fn unprocessed_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>>;
    /// One page of the events `filter` keeps, in its order.
    fn events_page<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        filter: &'a EventFilter,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>>;
    /// Up to `limit` events after position `after`, with the events at
    /// `pending` wherever they are; `None` once the session is gone.
    fn tail<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        after: Pos,
        pending: &'a [i64],
        limit: usize,
    ) -> BoxFuture<'a, Result<Option<Tail>, StoreError>>;
    /// Where a new stream of the session starts.
    fn head<'a>(&'a self, ws: &'a WorkspaceId, id: &'a SessionId) -> BoxFuture<'a, Result<Head, StoreError>>;
}

/// What an adapter implements to provide [`SessionEventReads`]; the methods are documented there.
pub trait NativeSessionEventReads: Send + Sync {
    fn events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
    ) -> impl Future<Output = Result<Vec<Event>, StoreError>> + Send;
    fn unprocessed_events(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
    ) -> impl Future<Output = Result<Vec<Event>, StoreError>> + Send;
    fn events_page(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        filter: &EventFilter,
        page: &Pagination,
    ) -> impl Future<Output = Result<Vec<Event>, StoreError>> + Send;
    fn tail(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        after: Pos,
        pending: &[i64],
        limit: usize,
    ) -> impl Future<Output = Result<Option<Tail>, StoreError>> + Send;
    fn head(&self, ws: &WorkspaceId, id: &SessionId)
    -> impl Future<Output = Result<Head, StoreError>> + Send;
}

impl<T: NativeSessionEventReads> SessionEventReads for T {
    fn events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(NativeSessionEventReads::events(self, ws, id))
    }
    fn unprocessed_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(NativeSessionEventReads::unprocessed_events(self, ws, id))
    }
    fn events_page<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        filter: &'a EventFilter,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(NativeSessionEventReads::events_page(self, ws, id, filter, page))
    }
    fn tail<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        after: Pos,
        pending: &'a [i64],
        limit: usize,
    ) -> BoxFuture<'a, Result<Option<Tail>, StoreError>> {
        Box::pin(NativeSessionEventReads::tail(self, ws, id, after, pending, limit))
    }
    fn head<'a>(&'a self, ws: &'a WorkspaceId, id: &'a SessionId) -> BoxFuture<'a, Result<Head, StoreError>> {
        Box::pin(NativeSessionEventReads::head(self, ws, id))
    }
}
