//! What a step takes from the session's log: the client events it reads,
//! the conversation they lead to, and the interrupts among them.

use std::collections::HashMap;

use sessions::events::{
    AgentEvent, ConfirmationResult, Event, EventId, EventPayload, SessionEvent, UserEvent,
};
use sessions::types::{Session, ThreadId};
use shared::Timestamp;

use super::grade::{active, conversation};
use super::team::{Thread, threads};
use crate::turn::compact::checkpoint;
use crate::turn::history::{History, Kept, Unanswered, rebuild, thread_events};
pub(super) use cut::cut_off;
use cut::thread_of;
use mcp::{is_mcp, refusal};

mod cut;
mod mcp;

/// The result a denied call gets when the client gave no reason.
const DENIED: &str = "The user denied this tool call.";

/// The text live posts for a built-in call an interrupt cut off (L-6).
pub(super) const INTERRUPTED: &str = "Tool execution was interrupted before completion. Please retry.";

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
    /// They are all the worker's `user.tool_result`s, which live stamps on
    /// arrival, before the turn runs again.
    pub(super) arrived: bool,
    /// Events the taken ones lead to: a `cloud` session's worker results,
    /// shown as the agent's (L-4), and the refusals of denied calls.
    pub(super) answers: Vec<Event>,
    /// A confirmation allowed a call, which the session's worker now runs.
    pub(super) allowed: bool,
    /// The thread the step runs; the primary for the session's own step.
    pub(super) thread: Thread,
    /// That thread's conversation, with the taken events in it.
    pub(super) history: History,
    /// The log as the step reads it: the taken events stamped.
    pub(super) read: Vec<Event>,
    /// The interrupts among the taken events.
    pub(super) interrupt: Interrupt,
    /// The threads that await the model or have an MCP call to run.
    pub(super) due: Vec<ThreadId>,
    /// Every thread with its conversation, for the session's own step.
    pub(super) all: Vec<(Thread, History)>,
}

fn is_interrupt(e: &Event) -> bool {
    matches!(e.payload, EventPayload::User(UserEvent::Interrupt {}))
}

/// Read `log` of `session` for `thread`'s step, or with `None` for the
/// session's own step, which alone takes the client's events.
pub(super) fn take(
    log: &[Event],
    session: &Session,
    cloud: bool,
    rubrics: &HashMap<String, String>,
    kept: &Kept,
    thread: Option<&ThreadId>,
) -> Taken {
    let primary = Thread::primary(session);
    let turn = &primary.turn();
    let queued: Vec<&Event> = log
        .iter()
        .filter(|e| thread.is_none() && e.processed_at.is_none() && e.payload.is_user_event())
        .collect();
    let taken: Vec<EventId> = queued.iter().map(|e| e.id.clone()).collect();
    let arrived = !queued.is_empty()
        && queued.iter().all(|e| matches!(e.payload, EventPayload::User(UserEvent::ToolResult { .. })));
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
                Some(refusal(log, turn, tool_use_id, why))
            }
            EventPayload::User(UserEvent::ToolConfirmation {
                tool_use_id,
                result: ConfirmationResult::Allow,
                ..
            }) => {
                // The loop runs an allowed MCP call; the worker the rest.
                allowed |= !is_mcp(log, tool_use_id);
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
    // An MCP call's allow holds once taken: a step that recorded some of its
    // results and runs again still calls the rest.
    let confirmed: Vec<&EventId> = log
        .iter()
        .filter_map(|e| match &e.payload {
            EventPayload::User(UserEvent::ToolConfirmation {
                tool_use_id,
                result: ConfirmationResult::Allow,
                ..
            }) if e.processed_at.is_none() || is_mcp(log, tool_use_id) => Some(tool_use_id),
            _ => None,
        })
        .collect();
    // An answer belongs to the thread of the call it answers.
    let answers: Vec<Event> =
        answers.into_iter().map(|a| Event { session_thread_id: thread_of(log, &a), ..a }).collect();
    let read: Vec<Event> = seen.into_iter().chain(answers.iter().cloned()).collect();
    let history_of = |t: &Thread| {
        let scope = (!t.primary).then(|| t.id.clone());
        let events = thread_events(&read, scope.as_ref());
        let view = checkpoint::view(&events, kept);
        let mut history = rebuild(&view.prompt, rubrics, kept);
        if let Some(outcome) = &t.grading {
            history = conversation(outcome, rubrics, history);
        }
        // The checkpoint first, then a grader's task (U2).
        if let Some(cp) = &view.checkpoint {
            history.messages.insert(0, cp.message());
        }
        for call in history.unanswered.iter_mut().filter(|u| confirmed.iter().any(|c| c.as_str() == u.id)) {
            call.asked = false;
        }
        history
    };
    let mut candidates = threads(session, &read);
    candidates.extend(active(&read).and_then(|o| o.thread(session)));
    let works = |h: &History| h.awaits_model() || h.unanswered.iter().any(Unanswered::for_loop);
    let all: Vec<(Thread, History)> = candidates.iter().map(|t| (t.clone(), history_of(t))).collect();
    let due: Vec<ThreadId> = all.iter().filter(|(_, h)| works(h)).map(|(t, _)| t.id.clone()).collect();
    let mine = thread.and_then(|id| all.iter().find(|(t, _)| t.id == *id)).cloned();
    let (thread, history) = mine.unwrap_or_else(|| (primary.clone(), history_of(&primary)));
    Taken { taken, arrived, answers, allowed, thread, history, read, interrupt, due, all }
}
