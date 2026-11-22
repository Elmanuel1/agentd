//! The reads only the loop makes: events with the `engine` object it stored
//! beside them, which no API read serves.

use shared::WorkspaceId;
use shared::ports::BoxFuture;
use std::future::Future;

use super::{NativeSessionEventReads, StoreError};
use crate::events::Event;
use crate::types::{SessionId, ThreadId};

/// Each event with the `engine` object the loop stored beside it.
pub type EngineEvents = Vec<(Event, Option<serde_json::Value>)>;

/// A session's log as the loop reads it. A session that does not exist,
/// which the caller has already read, has no events.
pub trait EngineReads: Send + Sync {
    /// Every event of the session, oldest first.
    fn engine_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>>;
    /// At least what one thread's step reads, oldest first: the thread's
    /// events (`None` is the primary's), the client events that answer its
    /// calls or answer none, every `session.*` event and the outcomes'
    /// evaluation spans.
    fn thread_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        thread: Option<&'a ThreadId>,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>>;
    /// The sum of the numbers the session's `engine` objects hold at `key`.
    fn engine_sum<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        key: &'a str,
    ) -> BoxFuture<'a, Result<u64, StoreError>>;
}

/// What an adapter implements to provide [`EngineReads`]; the methods are
/// documented there. The defaults read the whole log.
pub trait NativeEngineReads: NativeSessionEventReads {
    fn engine_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> impl Future<Output = Result<EngineEvents, StoreError>> + Send {
        async move {
            Ok(NativeSessionEventReads::events(self, ws, session)
                .await?
                .into_iter()
                .map(|e| (e, None))
                .collect())
        }
    }
    fn thread_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        _thread: Option<&ThreadId>,
    ) -> impl Future<Output = Result<EngineEvents, StoreError>> + Send {
        NativeEngineReads::engine_events(self, ws, session)
    }
    fn engine_sum(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        key: &str,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send {
        async move {
            let events = NativeEngineReads::engine_events(self, ws, session).await?;
            Ok(events.iter().filter_map(|(_, k)| k.as_ref()?.get(key)?.as_u64()).sum())
        }
    }
}

impl<T: NativeEngineReads> EngineReads for T {
    fn engine_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>> {
        Box::pin(NativeEngineReads::engine_events(self, ws, session))
    }
    fn thread_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        thread: Option<&'a ThreadId>,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>> {
        Box::pin(NativeEngineReads::thread_events(self, ws, session, thread))
    }
    fn engine_sum<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        key: &'a str,
    ) -> BoxFuture<'a, Result<u64, StoreError>> {
        Box::pin(NativeEngineReads::engine_sum(self, ws, session, key))
    }
}
