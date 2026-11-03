//! The harness's side of a session: emit, status, drain, usage, outcomes.

use async_trait::async_trait;
use shared::{ApiError, Timestamp, Usage, WorkspaceId};

use super::{map_store, HarnessContract, SessionsService};
use crate::events::{Event, EventPayload, SessionEvent, StopReason};
use crate::store::StoreError;
use crate::types::{OutcomeEvaluation, Session, SessionId, SessionStatus, ThreadId};

/// The same service implements it, so the event log and its broadcast have
/// one source.
#[async_trait]
impl HarnessContract for SessionsService {
    async fn session(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Session, ApiError> {
        self.store.get(ws, session).await.map_err(map_store)
    }

    async fn events(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Vec<Event>, ApiError> {
        self.events.events(ws, session).await.map_err(map_store)
    }

    async fn emit(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        thread: Option<&ThreadId>,
        payload: EventPayload,
    ) -> Result<Event, ApiError> {
        let mut ev = Event::new(payload);
        ev.session_thread_id = thread.cloned();
        // Agent-side events are processed by construction.
        ev.processed_at = Some(Timestamp::now());
        self.events.append_event(ws, session, ev.clone()).await.map_err(map_store)?;
        self.broadcast(session, &ev);
        Ok(ev)
    }

    async fn set_status(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        status: SessionStatus,
        stop_reason: Option<StopReason>,
    ) -> Result<(), ApiError> {
        self.mutate(ws, session, |s| {
            // Terminated is absorbing.
            if s.status == SessionStatus::Terminated {
                return Err(ApiError::conflict_invalid_request("session is terminated"));
            }
            s.status = status;
            Ok(())
        })
        .await?;
        let payload = match status {
            SessionStatus::Idle => EventPayload::Session(SessionEvent::StatusIdle {
                stop_reason: stop_reason.unwrap_or(StopReason::EndTurn {}),
            }),
            SessionStatus::Running => EventPayload::Session(SessionEvent::StatusRunning {}),
            SessionStatus::Rescheduling => EventPayload::Session(SessionEvent::StatusRescheduled {}),
            SessionStatus::Terminated => EventPayload::Session(SessionEvent::StatusTerminated {}),
        };
        self.emit(ws, session, None, payload).await?;
        if status == SessionStatus::Terminated {
            self.stop_sandbox(session).await;
        }
        Ok(())
    }

    async fn drain_user_events(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Vec<Event>, ApiError> {
        let events = self.events.unprocessed_events(ws, session).await.map_err(map_store)?;
        let mut drained = Vec::new();
        for mut ev in events {
            ev.processed_at = Some(Timestamp::now());
            self.events.replace_event(ws, session, ev.clone()).await.map_err(map_store)?;
            // The processed form is re-broadcast (documented: client events
            // appear twice, queued then processed).
            self.broadcast(session, &ev);
            drained.push(ev);
        }
        Ok(drained)
    }

    async fn add_usage(&self, ws: &WorkspaceId, session: &SessionId, usage: &Usage) -> Result<(), ApiError> {
        self.mutate(ws, session, |s| {
            s.usage.add(usage);
            Ok(())
        })
        .await
        .map(drop)
    }

    async fn record_outcome(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        eval: OutcomeEvaluation,
    ) -> Result<(), ApiError> {
        self.mutate(ws, session, |s| {
            // Replace the entry for this outcome or append.
            match s.outcome_evaluations.iter_mut().find(|e| e.outcome_id == eval.outcome_id) {
                Some(slot) => *slot = eval.clone(),
                None => s.outcome_evaluations.push(eval.clone()),
            }
            Ok(())
        })
        .await
        .map(drop)
    }
}

/// How often a harness write of the session row is retried after a
/// concurrent one changed it first (threads of one session write usage at
/// once).
const WRITE_ATTEMPTS: usize = 8;

impl SessionsService {
    /// Read the session, apply `f`, and write it back, again from a fresh
    /// read whenever another write got there first; the session as written.
    pub(super) async fn mutate(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        f: impl Fn(&mut Session) -> Result<(), ApiError> + Send,
    ) -> Result<Session, ApiError> {
        let mut attempt = 0;
        loop {
            let mut s = self.store.get(ws, session).await.map_err(map_store)?;
            let read = s.clone();
            f(&mut s)?;
            s.updated_at = Timestamp::now();
            match self.store.replace(ws, &read, s.clone()).await {
                Err(StoreError::Conflict) if attempt + 1 < WRITE_ATTEMPTS => attempt += 1,
                other => return other.map(|()| s).map_err(map_store),
            }
        }
    }
}
