//! An in-memory session and its event log.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use sessions::events::{Event, EventPayload, SessionEvent, StopReason};
use sessions::types::{OutcomeEvaluation, Session, SessionId, SessionStatus, ThreadId};
use sessions::EngineContract;
use shared::{ApiError, Timestamp, Usage, WorkspaceId};
use tokio::sync::broadcast;

/// One session in memory, with its event log; implements the engine's side
/// of the sessions service, and lets a test send client events.
#[derive(Clone)]
pub struct FakeSession {
    session: Arc<Mutex<Session>>,
    events: Arc<Mutex<Vec<Event>>>,
    live: broadcast::Sender<Event>,
}

impl std::fmt::Debug for FakeSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSession").finish_non_exhaustive()
    }
}

impl FakeSession {
    /// Hold `session`.
    pub fn new(session: Session) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
            events: Arc::default(),
            live: broadcast::channel(4096).0,
        }
    }

    /// The session as it is now.
    pub fn get(&self) -> Session {
        self.session.lock().clone()
    }

    /// Every event so far.
    pub fn log(&self) -> Vec<Event> {
        self.events.lock().clone()
    }

    /// The `type` of every event so far, in order.
    pub fn types(&self) -> Vec<String> {
        self.log()
            .iter()
            .map(|e| {
                serde_json::to_value(e)
                    .ok()
                    .and_then(|v| v.get("type").and_then(serde_json::Value::as_str).map(str::to_owned))
                    .unwrap_or_default()
            })
            .collect()
    }

    /// Queue a client event, as `POST …/events` does.
    pub fn send(&self, payload: impl Into<EventPayload>) -> Event {
        let ev = Event::new(payload.into());
        self.events.lock().push(ev.clone());
        let _ = self.live.send(ev.clone());
        ev
    }

    /// Every event from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.live.subscribe()
    }
}

#[async_trait]
impl EngineContract for FakeSession {
    async fn session(&self, _ws: &WorkspaceId, _id: &SessionId) -> Result<Session, ApiError> {
        Ok(self.get())
    }

    async fn events(&self, _ws: &WorkspaceId, _id: &SessionId) -> Result<Vec<Event>, ApiError> {
        Ok(self.log())
    }

    async fn emit(
        &self,
        _ws: &WorkspaceId,
        _id: &SessionId,
        thread: Option<&ThreadId>,
        payload: EventPayload,
    ) -> Result<Event, ApiError> {
        let mut ev = Event::new(payload);
        ev.processed_at = Some(Timestamp::now());
        ev.session_thread_id = thread.cloned();
        self.events.lock().push(ev.clone());
        let _ = self.live.send(ev.clone());
        Ok(ev)
    }

    async fn set_status(
        &self,
        ws: &WorkspaceId,
        id: &SessionId,
        status: SessionStatus,
        stop_reason: Option<StopReason>,
    ) -> Result<(), ApiError> {
        self.session.lock().status = status;
        let payload = match status {
            SessionStatus::Idle => {
                SessionEvent::StatusIdle { stop_reason: stop_reason.unwrap_or(StopReason::EndTurn {}) }
            }
            SessionStatus::Running => SessionEvent::StatusRunning {},
            SessionStatus::Rescheduling => SessionEvent::StatusRescheduled {},
            SessionStatus::Terminated => SessionEvent::StatusTerminated {},
        };
        self.emit(ws, id, None, payload.into()).await.map(|_| ())
    }

    async fn drain_user_events(&self, _ws: &WorkspaceId, _id: &SessionId) -> Result<Vec<Event>, ApiError> {
        let mut drained = Vec::new();
        for ev in self.events.lock().iter_mut() {
            if ev.payload.is_user_event() && ev.processed_at.is_none() {
                ev.processed_at = Some(Timestamp::now());
                drained.push(ev.clone());
            }
        }
        Ok(drained)
    }

    async fn has_unprocessed(&self, _ws: &WorkspaceId, _id: &SessionId) -> Result<bool, ApiError> {
        Ok(self.events.lock().iter().any(|e| e.payload.is_user_event() && e.processed_at.is_none()))
    }

    async fn add_usage(&self, _ws: &WorkspaceId, _id: &SessionId, usage: &Usage) -> Result<(), ApiError> {
        self.session.lock().usage.add(usage);
        Ok(())
    }

    async fn record_outcome(
        &self,
        _ws: &WorkspaceId,
        _id: &SessionId,
        eval: OutcomeEvaluation,
    ) -> Result<(), ApiError> {
        let mut s = self.session.lock();
        match s.outcome_evaluations.iter_mut().find(|e| e.outcome_id == eval.outcome_id) {
            Some(slot) => *slot = eval,
            None => s.outcome_evaluations.push(eval),
        }
        Ok(())
    }
}
