//! What a step takes from the session's log: the client events it reads,
//! the conversation they lead to, and the interrupts among them.

use std::collections::HashMap;

use sessions::events::{
    AgentEvent, ConfirmationResult, Event, EventId, EventPayload, SessionEvent, UserEvent,
};
use shared::{ContentBlock, Timestamp};

use super::reply::Turn;
use crate::turn::history::{rebuild, thread_events, History};

/// The result a denied call gets when the client gave no reason.
const DENIED: &str = "The user denied this tool call.";

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
    /// Events the taken ones lead to: a `cloud` session's worker results,
    /// shown as the agent's (L-4), and the refusals of denied calls.
    pub(super) answers: Vec<Event>,
    /// A confirmation allowed a call, which the session's worker now runs.
    pub(super) allowed: bool,
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
    let mut allowed = false;
    let answers: Vec<Event> = queued
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::User(UserEvent::ToolResult { tool_use_id, content, is_error }) if cloud => {
                Some(turn.event(AgentEvent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                }))
            }
            EventPayload::User(UserEvent::ToolConfirmation {
                tool_use_id,
                result: ConfirmationResult::Deny,
                deny_message,
            }) => {
                let why = deny_message.clone().unwrap_or_else(|| DENIED.to_owned());
                Some(turn.event(AgentEvent::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content: vec![ContentBlock::text(why)],
                    is_error: true,
                }))
            }
            EventPayload::User(UserEvent::ToolConfirmation { result: ConfirmationResult::Allow, .. }) => {
                allowed = true;
                None
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
    let confirmed: Vec<&EventId> = queued
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::User(UserEvent::ToolConfirmation {
                tool_use_id,
                result: ConfirmationResult::Allow,
                ..
            }) => Some(tool_use_id),
            _ => None,
        })
        .collect();
    let read: Vec<Event> = seen.into_iter().chain(answers.iter().cloned()).collect();
    let mut history = rebuild(&thread_events(&read, None), &HashMap::new());
    for call in history.unanswered.iter_mut().filter(|u| confirmed.iter().any(|c| c.as_str() == u.id)) {
        call.asked = false;
    }
    Taken { taken, answers, allowed, history, interrupt }
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
