//! A session's event log, as it is read.

use async_trait::async_trait;
use shared::{Pagination, WorkspaceId};

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

/// A session's event log, read.
#[async_trait]
pub trait SessionEventReads: Send + Sync {
    /// Every event of the session, oldest first, each with the `engine`
    /// object the loop stored beside it, which no API read serves.
    async fn engine_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> Result<Vec<(Event, Option<serde_json::Value>)>, StoreError> {
        Ok(self.events(ws, session).await?.into_iter().map(|e| (e, None)).collect())
    }
    /// Full event history, append order.
    async fn events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError>;
    /// The client events the agent has not taken yet, append order.
    async fn unprocessed_events(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Vec<Event>, StoreError>;
    /// One page of the events `filter` keeps, in its order.
    async fn events_page(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        filter: &EventFilter,
        page: &Pagination,
    ) -> Result<Vec<Event>, StoreError>;
    /// Up to `limit` events after position `after`, with the events at
    /// `pending` wherever they are; `None` once the session is gone.
    async fn tail(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        after: i64,
        pending: &[i64],
        limit: usize,
    ) -> Result<Option<Tail>, StoreError>;
    /// Where a new stream of the session starts.
    async fn head(&self, ws: &WorkspaceId, id: &SessionId) -> Result<Head, StoreError>;
}
