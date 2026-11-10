//! What a session waits on. While it is idle with `requires_action`, live
//! takes only an answer or an interrupt; a session's work token sends only
//! results, and only for its session's calls no result has answered yet.

use std::collections::HashSet;

use shared::{ApiError, Pagination, WorkspaceId};

use super::{map_store, parse_id, SessionsService};
use crate::events::{AgentEvent, Event, EventId, EventPayload, SessionEvent, StopReason, UserEvent};
use crate::store::{EventFilter, Order};
use crate::types::{Session, SessionStatus};

/// The events a session waiting on a tool still takes.
fn answers_or_interrupt(p: &EventPayload) -> bool {
    matches!(
        p,
        EventPayload::User(
            UserEvent::ToolConfirmation { .. }
                | UserEvent::CustomToolResult { .. }
                | UserEvent::ToolResult { .. }
                | UserEvent::Interrupt {}
        )
    )
}

/// The call a result answers, and whether it is a custom tool's.
fn answered(p: &EventPayload) -> Option<(&EventId, bool)> {
    match p {
        EventPayload::User(UserEvent::ToolResult { tool_use_id, .. }) => Some((tool_use_id, false)),
        EventPayload::User(UserEvent::CustomToolResult { custom_tool_use_id, .. }) => {
            Some((custom_tool_use_id, true))
        }
        _ => None,
    }
}

impl SessionsService {
    /// Live: while `session` is idle with `requires_action`, any event but
    /// an answer or an interrupt is refused, naming the waiting events.
    pub(super) async fn guard_waiting(
        &self,
        ws: &WorkspaceId,
        session: &Session,
        payloads: &[EventPayload],
    ) -> Result<(), ApiError> {
        let Some((i, refused)) = payloads.iter().enumerate().find(|(_, p)| !answers_or_interrupt(p)) else {
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

    /// A session's work token sends only `user.tool_result` for its
    /// session's `agent.tool_use` calls and `user.custom_tool_result` for its
    /// `agent.custom_tool_use` calls, each not yet answered.
    pub async fn guard_worker_results(
        &self,
        ws: &WorkspaceId,
        raw_id: &str,
        payloads: &[EventPayload],
    ) -> Result<(), ApiError> {
        let id = &parse_id(raw_id)?;
        let mut calls = Vec::with_capacity(payloads.len());
        for (i, p) in payloads.iter().enumerate() {
            let Some(call) = answered(p) else {
                return Err(ApiError::invalid_request(format!(
                    "events.{i}: a session's work token sends only user.tool_result and user.custom_tool_result"
                )));
            };
            calls.push(call);
        }
        let ids: Vec<EventId> = calls.iter().map(|(id, _)| (*id).clone()).collect();
        let found = self.events.tool_calls(ws, id, &ids).await.map_err(map_store)?;
        let is_call = |e: &Event, custom: bool| match &e.payload {
            EventPayload::Agent(AgentEvent::ToolUse { .. }) => !custom,
            EventPayload::Agent(AgentEvent::CustomToolUse { .. }) => custom,
            _ => false,
        };
        let done: HashSet<&EventId> = found
            .iter()
            .filter_map(|e| match &e.payload {
                EventPayload::User(UserEvent::ToolResult { tool_use_id: t, .. })
                | EventPayload::Agent(AgentEvent::ToolResult { tool_use_id: t, .. }) => Some(t),
                EventPayload::User(UserEvent::CustomToolResult { custom_tool_use_id: t, .. }) => Some(t),
                _ => None,
            })
            .collect();
        for (i, (call, custom)) in calls.iter().enumerate() {
            let open = found.iter().any(|e| e.id == **call && is_call(e, *custom)) && !done.contains(call);
            if !open {
                return Err(ApiError::invalid_request(format!(
                    "events.{i}: {call} is not a call of this session waiting on its result"
                )));
            }
        }
        Ok(())
    }
}
