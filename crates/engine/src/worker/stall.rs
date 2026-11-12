//! A grader that keeps ending its turn without its grade: after a few turns
//! its round fails, so a model that never grades cannot run model calls
//! without end.

use serde_json::json;
use sessions::events::{AgentEvent, Event, EventPayload, ModelUsage, SpanEvent};
use sessions::types::{OutcomeEvaluation, ThreadId};

use super::grade::{verdict, Outcome};
use super::turn::Turn;

/// Turns a grader may end without its verdict before its round fails.
const NUDGES: usize = 3;

/// The grader on `thread` ended `NUDGES` model calls with no tool call and
/// no grade (a grade ends the round). A call's tool calls come before its
/// `span.model_request_end`.
pub(super) fn stalled(log: &[Event], thread: &ThreadId) -> bool {
    let (mut ended, mut called) = (0, false);
    for e in log.iter().filter(|e| e.session_thread_id.as_ref() == Some(thread)) {
        match e.payload {
            EventPayload::Agent(AgentEvent::ToolUse { .. }) => called = true,
            EventPayload::Span(SpanEvent::ModelRequestEnd { .. }) => {
                ended += usize::from(!std::mem::replace(&mut called, false))
            }
            _ => {}
        }
    }
    ended >= NUDGES
}

/// The round's end when the grader stalled: it fails.
pub(super) fn no_verdict(primary: &Turn, outcome: &Outcome, usage: ModelUsage) -> (Event, OutcomeEvaluation) {
    let grade = json!({"result": "failed", "criteria": [{"criterion": "grade", "met": false,
        "finding": format!("The grader ended {NUDGES} turns without a grade.")}]});
    verdict(primary, outcome, &grade, usage)
}
