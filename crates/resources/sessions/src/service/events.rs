use shared::limits::{validate_range_u32, values};
use shared::{ApiError, AuthContext, Pagination};

use super::{SessionsService, map_store, parse_id};
use crate::events::{Event, EventPayload, UserEvent};
use crate::store::{Append, EventFilter, Wake};
use crate::types::{OutcomeId, Session};

impl SessionsService {
    /// `POST /v1/sessions/{id}/events` — queue client events in order.
    /// `environment` is an environment key's, which reaches its environment's sessions only.
    pub async fn send_events(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        environment: Option<&str>,
        payloads: Vec<EventPayload>,
    ) -> Result<Vec<Event>, ApiError> {
        let session = self.sendable(ctx, raw_id, environment, &payloads).await?;
        let out = queued(payloads);
        if out.is_empty() {
            return Ok(out);
        }
        let appended =
            self.events.append_events(&ctx.workspace, &session.id, &out, Wake::Step, Append::Always);
        appended.await.map_err(map_store)?;
        self.publish(&ctx.workspace, &session.id).await;
        Ok(out)
    }

    pub(super) async fn send_initial(
        &self,
        ctx: &AuthContext,
        session: &Session,
        initial: &[EventPayload],
    ) -> Result<(), ApiError> {
        if initial.is_empty() || session.takes_events().is_err() {
            return Ok(());
        }
        let (ws, id) = (&ctx.workspace, &session.id);
        let events = queued(initial.to_vec());
        if self.events.append_events(ws, id, &events, Wake::Step, Append::IfEmpty).await.map_err(map_store)? {
            self.publish(ws, id).await;
        }
        Ok(())
    }

    pub(super) async fn sendable(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        environment: Option<&str>,
        payloads: &[EventPayload],
    ) -> Result<Session, ApiError> {
        let id = parse_id(raw_id)?;
        let session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?.seen_from(environment)?;
        session.takes_events()?;
        validate_sendable("events", payloads)?;
        self.guard_waiting(&ctx.workspace, &session, payloads).await?;
        Ok(session)
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
        self.list_reads.events_page(&ctx.workspace, &id, filter, page).await.map_err(map_store)
    }
}

fn queued(payloads: Vec<EventPayload>) -> Vec<Event> {
    // define_outcome gets its server-assigned outcome id on echo.
    let stamped = payloads.into_iter().map(|p| match p {
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
    });
    stamped.map(Event::new).collect()
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
