//! What a session waits on. While it is idle with `requires_action`, live
//! takes only an answer or an interrupt; a session's work token sends only
//! results, and only for its session's calls no result has answered yet.

use std::collections::HashSet;

use shared::{ApiError, AuthContext, Pagination, WorkspaceId};

use super::{map_store, SessionsService};
use crate::events::{Event, EventId, EventPayload, SessionEvent, StopReason};
use crate::store::{EventFilter, Order};
use crate::types::{Session, SessionStatus};

impl SessionsService {
    /// Live: while `session` is idle with `requires_action`, any event but
    /// an answer or an interrupt is refused, naming the waiting events.
    pub(super) async fn guard_waiting(
        &self,
        ws: &WorkspaceId,
        session: &Session,
        payloads: &[EventPayload],
    ) -> Result<(), ApiError> {
        let Some((i, refused)) = payloads.iter().enumerate().find(|(_, p)| !p.answers_or_interrupts()) else {
            return Ok(());
        };
        if session.status != SessionStatus::Idle {
            return Ok(());
        }
        let filter = EventFilter {
            order: Order::Desc,
            types: vec!["session.status_idle".into()],
            ..EventFilter::default()
        };
        let last = self
            .events
            .events_page(ws, &session.id, &filter, &Pagination { limit: 1, offset: 0 })
            .await
            .map_err(map_store)?;
        let waiting = last.into_iter().find_map(|e| match e.payload {
            EventPayload::Session(SessionEvent::StatusIdle {
                stop_reason: StopReason::RequiresAction { event_ids },
                ..
            }) if !event_ids.is_empty() => Some(event_ids),
            _ => None,
        });
        let Some(waiting) = waiting else { return Ok(()) };
        let kind =
            serde_json::to_value(refused).ok().and_then(|v| v.get("type")?.as_str().map(str::to_owned));
        let ids: Vec<&str> = waiting.iter().map(EventId::as_str).collect();
        Err(ApiError::invalid_request(format!(
            "Invalid {} event at events[{i}]: waiting on responses to events [{}]; only \
             `user.tool_confirmation`, `user.custom_tool_result`, `user.tool_result`, or `user.interrupt` \
             may be sent",
            kind.unwrap_or_default(),
            ids.join(", ")
        )))
    }

    /// `POST /v1/sessions/{id}/events` from a session's work token: only
    /// `user.tool_result` for its session's `agent.tool_use` calls and
    /// `user.custom_tool_result` for its `agent.custom_tool_use` calls, each
    /// not yet answered.
    pub async fn send_results(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        environment: Option<&str>,
        payloads: Vec<EventPayload>,
    ) -> Result<Vec<Event>, ApiError> {
        let mut calls = Vec::with_capacity(payloads.len());
        for (i, p) in payloads.iter().enumerate() {
            let Some((call, custom)) = p.answered_call().filter(|_| p.is_user_event()) else {
                return Err(ApiError::invalid_request(format!(
                    "events.{i}: a session's work token sends only user.tool_result and user.custom_tool_result"
                )));
            };
            calls.push((call.clone(), custom));
        }
        let id = self.sendable(ctx, raw_id, environment, &payloads).await?;
        let ids: Vec<EventId> = calls.iter().map(|(call, _)| call.clone()).collect();
        let events: Vec<Event> = payloads.into_iter().map(Event::new).collect();
        let check = |found: &[Event]| {
            let done: HashSet<&EventId> =
                found.iter().filter_map(|e| e.payload.answered_call()).map(|(t, _)| t).collect();
            for (i, (call, custom)) in calls.iter().enumerate() {
                let first = calls.iter().take(i).all(|(earlier, _)| earlier != call);
                let open = first
                    && found.iter().any(|e| e.id == *call && e.payload.is_call(*custom))
                    && !done.contains(call);
                if !open {
                    return Err(ApiError::invalid_request(format!(
                        "events.{i}: {call} is not a call of this session waiting on its result"
                    )));
                }
            }
            Ok(())
        };
        self.events.append_answers(&ctx.workspace, &id, &ids, &events, &check).await.map_err(map_store)??;
        self.publish(&ctx.workspace, &id).await;
        Ok(events)
    }
}
