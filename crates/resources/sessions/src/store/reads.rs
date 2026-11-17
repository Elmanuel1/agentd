//! A session's event log, as it is read.

use shared::ports::BoxFuture;
use shared::{Pagination, WorkspaceId};
use std::future::Future;

use super::{EventFilter, StoreError};
use crate::events::Event;
use crate::types::SessionId;

/// A read of a session's log from a position.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tail {
    /// The session is archived.
    pub archived: bool,
    /// Each event with its position in the log, in log order.
    pub events: Vec<(i64, Event)>,
}

/// Where a new stream of a session starts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    /// The position of the session's latest event, 0 when it has none.
    pub seq: i64,
    /// The positions of its client events not processed yet, in log order.
    pub queued: Vec<i64>,
}

/// Each event with the `engine` object the loop stored beside it.
pub type EngineEvents = Vec<(Event, Option<serde_json::Value>)>;

/// A session's event log, read.
pub trait SessionEventReads: Send + Sync {
    /// Every event of the session, oldest first, each with the `engine`
    /// object the loop stored beside it, which no API read serves; none for a
    /// session that does not exist, which the caller has already read.
    fn engine_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>>;
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
        after: i64,
        pending: &'a [i64],
        limit: usize,
    ) -> BoxFuture<'a, Result<Option<Tail>, StoreError>>;
    /// Where a new stream of the session starts.
    fn head<'a>(&'a self, ws: &'a WorkspaceId, id: &'a SessionId) -> BoxFuture<'a, Result<Head, StoreError>>;
}

/// What an adapter implements to provide [`SessionEventReads`]; the methods are documented there.
pub trait SessionEventReadsImpl: Send + Sync {
    fn engine_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> impl Future<Output = Result<EngineEvents, StoreError>> + Send {
        async move { Ok(self.events(ws, session).await?.into_iter().map(|e| (e, None)).collect()) }
    }
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
        after: i64,
        pending: &[i64],
        limit: usize,
    ) -> impl Future<Output = Result<Option<Tail>, StoreError>> + Send;
    fn head(&self, ws: &WorkspaceId, id: &SessionId)
        -> impl Future<Output = Result<Head, StoreError>> + Send;
}

impl<T: SessionEventReadsImpl> SessionEventReads for T {
    fn engine_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>> {
        Box::pin(SessionEventReadsImpl::engine_events(self, ws, session))
    }
    fn events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(SessionEventReadsImpl::events(self, ws, id))
    }
    fn unprocessed_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(SessionEventReadsImpl::unprocessed_events(self, ws, id))
    }
    fn events_page<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        filter: &'a EventFilter,
        page: &'a Pagination,
    ) -> BoxFuture<'a, Result<Vec<Event>, StoreError>> {
        Box::pin(SessionEventReadsImpl::events_page(self, ws, id, filter, page))
    }
    fn tail<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        id: &'a SessionId,
        after: i64,
        pending: &'a [i64],
        limit: usize,
    ) -> BoxFuture<'a, Result<Option<Tail>, StoreError>> {
        Box::pin(SessionEventReadsImpl::tail(self, ws, id, after, pending, limit))
    }
    fn head<'a>(&'a self, ws: &'a WorkspaceId, id: &'a SessionId) -> BoxFuture<'a, Result<Head, StoreError>> {
        Box::pin(SessionEventReadsImpl::head(self, ws, id))
    }
}
