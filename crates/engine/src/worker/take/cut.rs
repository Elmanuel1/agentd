//! Where a taken answer belongs, and the results an interrupt posts.

use sessions::events::{Event, EventId};
use sessions::types::ThreadId;

use super::super::turn::Turn;
use super::INTERRUPTED;
use super::mcp::{answered, refusal};
use crate::turn::history::History;

/// The thread of the call `answer` answers.
pub(super) fn thread_of(log: &[Event], answer: &Event) -> Option<ThreadId> {
    let tool_use_id = answered(answer)?;
    log.iter().find(|e| e.id == *tool_use_id).and_then(|e| e.session_thread_id.clone())
}

/// The error results live posts for the built-in calls an interrupt cut off.
pub(in crate::worker) fn cut_off(turn: &Turn, history: &History, log: &[Event]) -> Vec<Event> {
    history
        .unanswered
        .iter()
        .filter(|u| u.for_worker() || u.for_loop())
        .filter_map(|u| EventId::parse(&u.id).ok())
        .map(|id| refusal(log, turn, &id, INTERRUPTED.to_owned()))
        .collect()
}
