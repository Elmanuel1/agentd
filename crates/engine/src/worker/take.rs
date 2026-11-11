//! What a step takes from the session's log: the client events it reads,
//! the conversation they lead to, and the interrupts among them.

use std::collections::HashMap;

use sessions::events::{AgentEvent, Event, EventId, EventPayload, SessionEvent, UserEvent};
use shared::{ContentBlock, Timestamp};

use super::reply::Turn;
use crate::turn::history::{rebuild, thread_events, History};

/// The text live posts for a built-in call an interrupt cut off (L-6).
const INTERRUPTED: &str = "Tool execution was interrupted before completion. Please retry.";

/// How the client interrupted the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Interrupt {
    /// It did not.
    None,
    /// Once: the turn stops (L-6).
    First,
    /// Again in the same turn: the session's work is force-stopped too (L-7).
    Second,
}

/// The log as the step reads it.
pub(super) struct Taken {
    /// The client events not yet processed, which the step stamps.
    pub(super) taken: Vec<EventId>,
    /// A `cloud` session's worker results, shown as the agent's (L-4).
    pub(super) mirrored: Vec<Event>,
    /// The primary thread's conversation, with the taken events in it.
    pub(super) history: History,
    /// The interrupts among the taken events.
    pub(super) interrupt: Interrupt,
}

fn is_interrupt(e: &Event) -> bool {
    matches!(e.payload, EventPayload::User(UserEvent::Interrupt {}))
}

/// Read `log` for `turn`.
pub(super) fn take(log: &[Event], turn: &Turn, cloud: bool) -> Taken {
    let queued: Vec<&Event> =
        log.iter().filter(|e| e.processed_at.is_none() && e.payload.is_user_event()).collect();
    let taken: Vec<EventId> = queued.iter().map(|e| e.id.clone()).collect();
    let now = Timestamp::now();
    let seen: Vec<Event> = log
        .iter()
        .map(|e| Event { processed_at: e.processed_at.or(taken.contains(&e.id).then_some(now)), ..e.clone() })
        .collect();
    let mirrored = queued
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::User(UserEvent::ToolResult { tool_use_id, content, is_error }) if cloud => {
                Some(turn.event(AgentEvent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }))
            }
            _ => None,
        })
        .collect();
    let since_running = log
        .iter()
        .rev()
        .take_while(|e| !matches!(e.payload, EventPayload::Session(SessionEvent::StatusRunning {})));
    let earlier = since_running.filter(|e| e.processed_at.is_some()).any(is_interrupt);
    let interrupt = match (queued.iter().filter(|e| is_interrupt(e)).count(), earlier) {
        (0, _) => Interrupt::None,
        (1, false) => Interrupt::First,
        _ => Interrupt::Second,
    };
    let history = rebuild(&thread_events(&seen, None), &HashMap::new());
    Taken { taken, mirrored, history, interrupt }
}

/// The error results live posts for the built-in calls an interrupt cut off.
pub(super) fn cut_off(turn: &Turn, history: &History) -> Vec<Event> {
    history
        .unanswered
        .iter()
        .filter(|u| u.for_worker())
        .filter_map(|u| EventId::parse(&u.id).ok())
        .map(|tool_use_id| {
            turn.event(AgentEvent::ToolResult {
                tool_use_id,
                content: vec![ContentBlock::text(INTERRUPTED)],
                is_error: true,
            })
        })
        .collect()
}
