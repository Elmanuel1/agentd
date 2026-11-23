//! The reads only the loop makes: events with the `engine` object it stored
//! beside them, which no API read serves. A step reads each thread from its
//! latest checkpoint, the snapshot the thread resumes from.

use shared::WorkspaceId;
use shared::ports::BoxFuture;
use std::future::Future;

use super::{NativeSessionEventReads, StoreError};
use crate::events::{Event, EventPayload, SpanEvent};
use crate::types::{SessionId, ThreadId};

/// Each event with the `engine` object the loop stored beside it.
pub type EngineEvents = Vec<(Event, Option<serde_json::Value>)>;

/// What a session's model calls add up to, every thread's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Calls {
    /// Tokens read from the cache, by the calls that did not fail.
    pub cache_read_input_tokens: u64,
    /// Input tokens, cache writes included, by the calls that did not fail.
    pub input_tokens: u64,
    /// Output tokens, by the calls that did not fail.
    pub output_tokens: u64,
    /// The calls that did not fail with no cost kept beside them.
    pub unpriced: u64,
    /// The costs kept beside every call.
    pub cost: u64,
}

/// How a thread's step reads its thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadRead {
    /// From the thread's first event, not its latest checkpoint.
    pub whole: bool,
}

/// A session's log as the loop reads it. A session that does not exist,
/// which the caller has already read, has no events.
pub trait EngineReads: Send + Sync {
    /// At least what the session's own step reads, oldest first: each
    /// thread's events from its latest checkpoint's covered event, every
    /// `session.*` event, the outcomes' evaluation spans and the client
    /// events that answer no call. A processed tool result may come with
    /// its content left out, as the step builds no prompt.
    fn step_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>>;
    /// At least what one thread's step reads: as [`Self::step_events`], with
    /// only this thread's own events (`None` is the primary's), as `how`
    /// says, with the answers to its calls, every event whole.
    fn thread_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        thread: Option<&'a ThreadId>,
        how: ThreadRead,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>>;
    /// The session's `span.model_request_end`s added up, with the costs
    /// their `engine` objects hold at `cost`.
    fn calls<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        cost: &'a str,
    ) -> BoxFuture<'a, Result<Calls, StoreError>>;
}

/// What an adapter implements to provide [`EngineReads`]; the methods are
/// documented there. The defaults read the whole log.
pub trait NativeEngineReads: NativeSessionEventReads {
    /// Every event of the session, oldest first, each with its kept object.
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
    fn step_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
    ) -> impl Future<Output = Result<EngineEvents, StoreError>> + Send {
        NativeEngineReads::engine_events(self, ws, session)
    }
    fn thread_events(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        _thread: Option<&ThreadId>,
        _how: ThreadRead,
    ) -> impl Future<Output = Result<EngineEvents, StoreError>> + Send {
        NativeEngineReads::engine_events(self, ws, session)
    }
    fn calls(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        cost: &str,
    ) -> impl Future<Output = Result<Calls, StoreError>> + Send {
        async move {
            let events = NativeEngineReads::engine_events(self, ws, session).await?;
            Ok(events.iter().fold(Calls::default(), |mut c, (e, kept)| {
                let kept = kept.as_ref().and_then(|k| k.get(cost)?.as_u64());
                c.cost += kept.unwrap_or_default();
                if let EventPayload::Span(SpanEvent::ModelRequestEnd {
                    model_usage, is_error: false, ..
                }) = &e.payload
                {
                    let t = &model_usage.tokens;
                    c.cache_read_input_tokens += t.cache_read_input_tokens;
                    c.input_tokens += t.input_tokens + t.cache_creation_input_tokens;
                    c.output_tokens += t.output_tokens;
                    c.unpriced += u64::from(kept.is_none());
                }
                c
            }))
        }
    }
}

impl<T: NativeEngineReads> EngineReads for T {
    fn step_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>> {
        Box::pin(NativeEngineReads::step_events(self, ws, session))
    }
    fn thread_events<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        thread: Option<&'a ThreadId>,
        how: ThreadRead,
    ) -> BoxFuture<'a, Result<EngineEvents, StoreError>> {
        Box::pin(NativeEngineReads::thread_events(self, ws, session, thread, how))
    }
    fn calls<'a>(
        &'a self,
        ws: &'a WorkspaceId,
        session: &'a SessionId,
        cost: &'a str,
    ) -> BoxFuture<'a, Result<Calls, StoreError>> {
        Box::pin(NativeEngineReads::calls(self, ws, session, cost))
    }
}
