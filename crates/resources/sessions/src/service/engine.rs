//! The engine's side of a session: emit, status, drain, usage, outcomes.

use async_trait::async_trait;
use shared::{ApiError, Timestamp, Usage, WorkspaceId};

use super::{map_store, EngineContract, SessionsService};
use crate::events::{Event, EventPayload, SessionEvent, StopReason};
use crate::types::{OutcomeEvaluation, SessionId, SessionStatus};

/// The engine contract is implemented by the same service (single source of
/// truth for the event log + broadcast), consumed by `engine`.
#[async_trait]
impl EngineContract for SessionsService {
    async fn emit(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        payload: EventPayload,
    ) -> Result<Event, ApiError> {
        let mut ev = Event::new(payload);
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
        let mut s = self.store.get(ws, session).await.map_err(map_store)?;
        // Terminated is absorbing.
        if s.status == SessionStatus::Terminated {
            return Err(ApiError::conflict_invalid_request("session is terminated"));
        }
        s.status = status;
        s.updated_at = Timestamp::now();
        self.store.replace(ws, s).await.map_err(map_store)?;
        let payload = match status {
            SessionStatus::Idle => EventPayload::Session(SessionEvent::StatusIdle {
                stop_reason: stop_reason.unwrap_or(StopReason::EndTurn {}),
            }),
            SessionStatus::Running => EventPayload::Session(SessionEvent::StatusRunning {}),
            SessionStatus::Rescheduling => EventPayload::Session(SessionEvent::StatusRescheduled {}),
            SessionStatus::Terminated => EventPayload::Session(SessionEvent::StatusTerminated {}),
        };
        self.emit(ws, session, payload).await?;
        Ok(())
    }

    async fn drain_user_events(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Vec<Event>, ApiError> {
        let events = self.events.events(ws, session).await.map_err(map_store)?;
        let mut drained = Vec::new();
        for mut ev in events {
            if ev.payload.is_user_event() && ev.processed_at.is_none() {
                ev.processed_at = Some(Timestamp::now());
                self.events.replace_event(ws, session, ev.clone()).await.map_err(map_store)?;
                // The processed form is re-broadcast (documented: client
                // events appear twice — queued then processed).
                self.broadcast(session, &ev);
                drained.push(ev);
            }
        }
        Ok(drained)
    }

    async fn add_usage(&self, ws: &WorkspaceId, session: &SessionId, usage: &Usage) -> Result<(), ApiError> {
        let mut s = self.store.get(ws, session).await.map_err(map_store)?;
        s.usage.add(usage);
        s.updated_at = Timestamp::now();
        self.store.replace(ws, s).await.map_err(map_store)
    }

    async fn record_outcome(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        eval: OutcomeEvaluation,
    ) -> Result<(), ApiError> {
        let mut s = self.store.get(ws, session).await.map_err(map_store)?;
        // Replace the entry for this outcome or append.
        match s.outcome_evaluations.iter_mut().find(|e| e.outcome_id == eval.outcome_id) {
            Some(slot) => *slot = eval,
            None => s.outcome_evaluations.push(eval),
        }
        s.updated_at = Timestamp::now();
        self.store.replace(ws, s).await.map_err(map_store)
    }
}
