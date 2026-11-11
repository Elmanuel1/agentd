use shared::limits::{validate_range_u32, values};
use shared::{ApiError, AuthContext, Pagination};

use super::{guard_writable, map_store, parse_id, SessionsService};
use crate::events::{Event, EventPayload, UserEvent};
use crate::store::EventFilter;
use crate::types::{OutcomeId, SessionId, SessionStatus};

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
        let id = self.sendable(ctx, raw_id, environment, &payloads).await?;
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
            out.push(Event::new(p));
        }
        if out.is_empty() {
            return Ok(out);
        }
        self.events.append_events(&ctx.workspace, &id, &out).await.map_err(map_store)?;
        self.publish(&ctx.workspace, &id).await;
        Ok(out)
    }

    pub(super) async fn sendable(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        environment: Option<&str>,
        payloads: &[EventPayload],
    ) -> Result<SessionId, ApiError> {
        let id = parse_id(raw_id)?;
        let session = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?.seen_from(environment)?;
        guard_writable(&session)?;
        if session.status == SessionStatus::Terminated {
            return Err(ApiError::conflict_invalid_request("cannot send events to a terminated session"));
        }
        validate_sendable("events", payloads)?;
        self.guard_waiting(&ctx.workspace, &session, payloads).await?;
        Ok(id)
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
