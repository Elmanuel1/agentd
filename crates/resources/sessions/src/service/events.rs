use async_trait::async_trait;
use shared::limits::{validate_range_u32, values};
use shared::{ApiError, AuthContext, Timestamp, Usage, WorkspaceId};
use tokio::sync::broadcast;

use super::{guard_writable, map_store, parse_id, EngineContract, SessionsService};
use crate::events::{Event, EventPayload, StopReason};
use crate::types::{OutcomeEvaluation, OutcomeId, SessionId, SessionStatus};

impl SessionsService {
    /// `POST /v1/sessions/{id}/events` — queue client events in order.
    pub async fn send_events(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        payloads: Vec<EventPayload>,
    ) -> Result<Vec<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        let session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        guard_writable(&session)?;
        if session.status == SessionStatus::Terminated {
            return Err(ApiError::conflict_invalid_request("cannot send events to a terminated session"));
        }
        // Validate each payload with the live field-path error convention.
        for (i, p) in payloads.iter().enumerate() {
            if let EventPayload::UserDefineOutcome { max_iterations: Some(n), .. } = p {
                validate_range_u32(
                    &format!("events.{i}.user_define_outcome.max_iterations"),
                    *n,
                    values::OUTCOME_MAX_ITERATIONS_MIN,
                    values::OUTCOME_MAX_ITERATIONS_MAX,
                )?;
            }
            if !p.is_user_event() {
                return Err(ApiError::invalid_request(format!(
                    "events.{i}: only user.* and system.message events can be sent"
                )));
            }
        }
        let mut out = Vec::with_capacity(payloads.len());
        for p in payloads {
            // define_outcome gets its server-assigned outcome id on echo.
            let p = match p {
                EventPayload::UserDefineOutcome { description, rubric, max_iterations, outcome_id: _ } => {
                    EventPayload::UserDefineOutcome {
                        description,
                        rubric,
                        max_iterations,
                        outcome_id: Some(OutcomeId::generate()),
                    }
                }
                other => other,
            };
            let ev = Event::new(p);
            self.store.append_event(&ctx.workspace, &id, ev.clone()).await.map_err(map_store)?;
            self.broadcast(&id, &ev);
            out.push(ev);
        }
        Ok(out)
    }

    /// `GET /v1/sessions/{id}/events` — full history (reconnect/dedupe
    /// source; the SSE stream has no replay).
    pub async fn list_events(&self, ctx: &AuthContext, raw_id: &str) -> Result<Vec<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.events(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// Subscribe to live events (`GET /v1/sessions/{id}/events/stream`).
    pub async fn subscribe(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<broadcast::Receiver<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        // Existence check under the caller's workspace.
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        Ok(self.channel(&id).subscribe())
    }

    fn channel(&self, id: &SessionId) -> broadcast::Sender<Event> {
        self.channels.lock().entry(id.clone()).or_insert_with(|| broadcast::channel(1024).0).clone()
    }

    fn broadcast(&self, id: &SessionId, ev: &Event) {
        // No receiver = no error; stream is fire-and-forget.
        let _ = self.channel(id).send(ev.clone());
    }
}

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
        self.store.append_event(ws, session, ev.clone()).await.map_err(map_store)?;
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
            SessionStatus::Idle => {
                EventPayload::SessionStatusIdle { stop_reason: stop_reason.unwrap_or(StopReason::EndTurn {}) }
            }
            SessionStatus::Running => EventPayload::SessionStatusRunning {},
            SessionStatus::Rescheduling => EventPayload::SessionStatusRescheduled {},
            SessionStatus::Terminated => EventPayload::SessionStatusTerminated {},
        };
        self.emit(ws, session, payload).await?;
        Ok(())
    }

    async fn drain_user_events(&self, ws: &WorkspaceId, session: &SessionId) -> Result<Vec<Event>, ApiError> {
        let events = self.store.events(ws, session).await.map_err(map_store)?;
        let mut drained = Vec::new();
        for mut ev in events {
            if ev.payload.is_user_event() && ev.processed_at.is_none() {
                ev.processed_at = Some(Timestamp::now());
                self.store.replace_event(ws, session, ev.clone()).await.map_err(map_store)?;
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
