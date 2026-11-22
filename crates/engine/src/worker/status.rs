//! A thread's state as its lifecycle events in the log say it: working,
//! mid-turn, or parked on what it waits for.

use std::collections::HashSet;

use sessions::events::{AgentEvent, Event, EventId, EventPayload, SessionEvent, StopReason};
use sessions::types::ThreadId;

use super::team::Thread;
use crate::turn::history::thread_events;

/// A thread's last status in `log`: running (`Some(None)`), idle and why
/// (`Some(Some(_))`), or none yet.
fn status<'a>(log: &'a [Event], thread: &ThreadId) -> Option<Option<&'a StopReason>> {
    log.iter().rev().filter(|e| e.session_thread_id.as_ref() == Some(thread)).find_map(|e| match &e.payload {
        EventPayload::Session(SessionEvent::ThreadStatusRunning { .. }) => Some(None),
        EventPayload::Session(SessionEvent::ThreadStatusIdle { stop_reason, .. }) => Some(Some(stop_reason)),
        _ => None,
    })
}

/// A thread is at work: its last status in `log` is running.
pub(super) fn busy(log: &[Event], thread: &ThreadId) -> bool {
    matches!(status(log, thread), Some(None))
}

/// A thread is mid-turn: running, or idle on a call it waits for, not on
/// the end of its turn.
pub(super) fn unfinished(log: &[Event], thread: &ThreadId) -> bool {
    match status(log, thread) {
        Some(Some(reason)) => !matches!(reason, StopReason::EndTurn {}),
        Some(None) => true,
        None => false,
    }
}

/// What a thread waits for while idle: the calls it stopped on, or the
/// budget (`None` while it works or has ended its turn).
pub(super) fn parked(log: &[Event], thread: &ThreadId) -> Option<&'static str> {
    match status(log, thread) {
        Some(Some(StopReason::RequiresAction { .. })) => Some("requires_action"),
        Some(Some(StopReason::BudgetReached {})) => Some("budget_reached"),
        _ => None,
    }
}

/// The calls thread `thread` waits on, when it stopped on some.
pub(super) fn awaited(log: &[Event], thread: &ThreadId) -> Vec<EventId> {
    match status(log, thread) {
        Some(Some(StopReason::RequiresAction { event_ids })) => event_ids.clone(),
        _ => Vec::new(),
    }
}

/// `created` opens the thread `sent` sends its first message to.
pub(super) fn opens(created: &Event, sent: &Event) -> bool {
    matches!(created.payload, EventPayload::Session(SessionEvent::ThreadCreated { .. }))
        && matches!(&sent.payload, EventPayload::Agent(AgentEvent::ThreadMessageSent { to_session_thread_id, .. })
            if created.session_thread_id.as_ref() == Some(to_session_thread_id))
}

/// `e` is a coordinator's message to one of its threads.
pub(super) fn is_send(e: &Event) -> bool {
    matches!(e.payload, EventPayload::Agent(AgentEvent::ThreadMessageSent { .. }))
}

/// Of `later`, the events not in `read`, one is `thread`'s: its client's
/// message, or a result for one of its calls. Events for other threads do
/// not keep its turn from ending.
pub(super) fn arrived_for(thread: &Thread, read: &[Event], later: &[Event]) -> bool {
    let known: HashSet<&EventId> = read.iter().map(|e| &e.id).collect();
    let fresh: Vec<Event> = later.iter().filter(|e| !known.contains(&e.id)).cloned().collect();
    let all: Vec<Event> = read.iter().cloned().chain(fresh.iter().cloned()).collect();
    let scope = (!thread.primary).then(|| thread.id.clone());
    thread_events(&all, scope.as_ref()).iter().any(|e| fresh.iter().any(|f| f.id == e.id))
}
