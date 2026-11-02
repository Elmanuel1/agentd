use shared::limits::{validate_range_u32, values};
use shared::{ApiError, AuthContext, Pagination};
use tokio::sync::broadcast;

use super::{guard_writable, map_store, parse_id, SessionsService};
use crate::events::{Event, EventPayload, UserEvent};
use crate::store::EventFilter;
use crate::types::{OutcomeId, SessionId, SessionStatus};

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
        validate_sendable("events", &payloads)?;
        let mut out = Vec::with_capacity(payloads.len());
        for p in payloads {
            // define_outcome gets its server-assigned outcome id on echo.
            let p = match p {
                EventPayload::User(UserEvent::DefineOutcome {
                    description,
                    rubric,
                    max_iterations,
                    outcome_id: _,
                }) => EventPayload::User(UserEvent::DefineOutcome {
                    description,
                    rubric,
                    max_iterations,
                    outcome_id: Some(OutcomeId::generate()),
                }),
                other => other,
            };
            let ev = Event::new(p);
            self.events.append_event(&ctx.workspace, &id, ev.clone()).await.map_err(map_store)?;
            self.broadcast(&id, &ev);
            out.push(ev);
        }
        if let Some(trigger) = self.trigger.get() {
            trigger.wake(ctx, &id);
        }
        Ok(out)
    }

    /// `GET /v1/sessions/{id}/events`: one page of the history (the
    /// reconnect and dedupe source; the SSE stream has no replay).
    pub async fn list_events(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        filter: &EventFilter,
        page: &Pagination,
    ) -> Result<Vec<Event>, ApiError> {
        let id = parse_id(raw_id)?;
        self.events.events_page(&ctx.workspace, &id, filter, page).await.map_err(map_store)
    }

    /// Subscribe to live events (`GET /v1/sessions/{id}/events/stream`).
    pub async fn subscribe(&self, ctx: &AuthContext, raw_id: &str) -> Result<Subscription, ApiError> {
        let id = parse_id(raw_id)?;
        // Existence check under the caller's workspace.
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        let rx =
            self.channels.lock().entry(id.clone()).or_insert_with(|| broadcast::channel(1024).0).subscribe();
        Ok(Subscription { rx, channels: self.channels.clone(), id })
    }

    /// Send `ev` to the session's listeners. A channel no one listens to is
    /// dropped, so the map holds only sessions someone is streaming.
    pub(super) fn broadcast(&self, id: &SessionId, ev: &Event) {
        let mut channels = self.channels.lock();
        if let Some(tx) = channels.get(id) {
            if tx.send(ev.clone()).is_err() {
                channels.remove(id);
            }
        }
    }
}

/// One listener on a session's events; the session's channel goes when its
/// last listener does.
#[derive(Debug)]
pub struct Subscription {
    rx: broadcast::Receiver<Event>,
    channels: super::Channels,
    id: SessionId,
}

impl Subscription {
    /// The next event; an error ends the stream (deleted, or fell behind).
    pub async fn recv(&mut self) -> Result<Event, broadcast::error::RecvError> {
        self.rx.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut channels = self.channels.lock();
        // This receiver is still counted while it drops.
        if channels.get(&self.id).is_some_and(|tx| tx.receiver_count() <= 1) {
            channels.remove(&self.id);
        }
    }
}

/// Each payload a client may send, with the live field-path error convention
/// (`events.0.user_define_outcome.max_iterations`).
pub(super) fn validate_sendable(field: &str, payloads: &[EventPayload]) -> Result<(), ApiError> {
    for (i, p) in payloads.iter().enumerate() {
        if let EventPayload::User(UserEvent::DefineOutcome { max_iterations: Some(n), .. }) = p {
            validate_range_u32(
                &format!("{field}.{i}.user_define_outcome.max_iterations"),
                *n,
                values::OUTCOME_MAX_ITERATIONS_MIN,
                values::OUTCOME_MAX_ITERATIONS_MAX,
            )?;
        }
        if !p.is_user_event() {
            return Err(ApiError::invalid_request(format!(
                "{field}.{i}: only user.* and system.message events can be sent"
            )));
        }
    }
    Ok(())
}
