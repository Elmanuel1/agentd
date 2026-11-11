//! Who a step's events belong to, and how one write's events are timed.

use sessions::events::{Event, EventId, EventPayload, SessionEvent, SpanEvent, StopReason};
use sessions::types::{Session, ThreadId};
use shared::Timestamp;

/// Who a step's events belong to.
pub(super) struct Turn {
    /// The thread the call was for.
    pub(super) thread: ThreadId,
    /// The agent that thread runs.
    pub(super) agent: String,
    /// A thread other than the primary: every event names it.
    pub(super) scoped: bool,
}

impl Turn {
    /// A stamped event of this turn's thread.
    pub(super) fn event(&self, payload: impl Into<EventPayload>) -> Event {
        let mut e = Event::new(payload.into());
        e.processed_at = Some(Timestamp::now());
        e.session_thread_id = self.scoped.then(|| self.thread.clone());
        e
    }

    /// A lifecycle event about this turn's thread, which names it.
    pub(super) fn lifecycle(&self, payload: SessionEvent) -> Event {
        let mut e = self.event(payload);
        e.session_thread_id = Some(self.thread.clone());
        e
    }

    /// The session and its thread start running.
    pub(super) fn running(&self) -> Vec<Event> {
        vec![
            self.event(SessionEvent::StatusRunning {}),
            self.lifecycle(SessionEvent::ThreadStatusRunning { agent_name: self.agent.clone() }),
        ]
    }

    /// A call that ended with no reply: its end, and the error when it failed.
    pub(super) fn failed(&self, start: &EventId, e: Option<&crate::model::ModelError>) -> Vec<Event> {
        let end = self.event(SpanEvent::ModelRequestEnd {
            model_request_start_id: start.clone(),
            is_error: true,
            model_usage: Default::default(),
        });
        let error = e.map(|e| {
            self.event(SessionEvent::Error {
                error: serde_json::json!({"type": "model_error", "message": e.to_string()}),
            })
        });
        std::iter::once(end).chain(error).collect()
    }

    /// The turn ends as `stop` says: the thread, then the session's usage as
    /// `session` holds it after the step, then the session.
    pub(super) fn idle(&self, session: &Session, stop: StopReason) -> Vec<Event> {
        vec![
            self.lifecycle(thread_idle(self, stop.clone())),
            self.event(SessionEvent::Usage { usage: session.usage.clone(), budget: session.budget.clone() }),
            self.event(SessionEvent::StatusIdle { stop_reason: stop }),
        ]
    }
}

/// A thread's idle, as its lifecycle event says it.
pub(super) fn thread_idle(turn: &Turn, stop: StopReason) -> SessionEvent {
    SessionEvent::ThreadStatusIdle { agent_name: turn.agent.clone(), stop_reason: stop, stop_details: None }
}

/// The events one write lands, timed in order around the stamp of the
/// client events it reads, since the events list orders by `processed_at`.
pub(super) struct Batch {
    pub(super) read: Vec<EventId>,
    pub(super) at: Timestamp,
    pub(super) events: Vec<Event>,
}

/// `before`, then the stamp of `read`, then `after`, a microsecond apart.
pub(super) fn batch(read: Vec<EventId>, before: Vec<Event>, after: Vec<Event>) -> Batch {
    let start = Timestamp::now().as_datetime();
    let tick = |i: usize| {
        let micros = i64::try_from(i).unwrap_or(i64::MAX);
        Timestamp::from_datetime(start + chrono::Duration::microseconds(micros))
    };
    let n = before.len();
    let timed = |i: usize, e: Event| Event { processed_at: Some(tick(i)), ..e };
    let first = before.into_iter().enumerate().map(|(i, e)| timed(i, e));
    let then = after.into_iter().enumerate().map(|(j, e)| timed(n + 1 + j, e));
    Batch { read, at: tick(n), events: first.chain(then).collect() }
}
