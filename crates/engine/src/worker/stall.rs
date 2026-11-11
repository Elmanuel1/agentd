//! A grader that keeps ending its turn without its verdict: after a few
//! turns its round fails, so a model that never calls the tool cannot run
//! model calls without end.

use serde_json::json;
use sessions::events::{AgentEvent, Event, EventPayload, ModelUsage};
use sessions::types::{OutcomeEvaluation, ThreadId};

use super::grade::{verdict, Outcome, VERDICT};
use super::turn::Turn;

/// Turns a grader may end without its verdict before its round fails.
const NUDGES: usize = 3;

/// The grader on `thread` ended `NUDGES` turns without its verdict; each
/// left its text as an `agent.message` on its thread.
pub(super) fn stalled(log: &[Event], thread: &ThreadId) -> bool {
    let ended = log.iter().filter(|e| {
        e.session_thread_id.as_ref() == Some(thread)
            && matches!(e.payload, EventPayload::Agent(AgentEvent::Message { .. }))
    });
    ended.count() >= NUDGES
}

/// The round's end when the grader stalled: it fails.
pub(super) fn no_verdict(primary: &Turn, outcome: &Outcome, usage: ModelUsage) -> (Event, OutcomeEvaluation) {
    let input = json!({"result": "failed", "findings": [{
        "criterion": "verdict", "finding": format!("The grader ended {NUDGES} turns without calling `{VERDICT}`.")}]});
    verdict(primary, outcome, &input, usage)
}
