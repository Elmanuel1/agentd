//! A grader that keeps ending its turn without its grade: after a few turns
//! its round fails, so a model that never grades cannot run model calls
//! without end.

use serde_json::json;
use sessions::events::{AgentEvent, Event, EventPayload, ModelUsage, SpanEvent};
use sessions::types::{OutcomeEvaluation, Session, ThreadId};

use super::grade::{Outcome, verdict};
use super::team::Thread;
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
            // A compaction's call is not a turn the grader ended.
            EventPayload::Agent(AgentEvent::ThreadContextCompacted {}) => ended = ended.saturating_sub(1),
            _ => {}
        }
    }
    ended >= NUDGES
}

/// A grader that stopped on the budget or its retries ends its round failed,
/// so the round does not stay open with no one to grade it; `why` says which.
pub(super) fn abandoned(session: &Session, thread: &Thread, why: &str) -> Option<(Event, OutcomeEvaluation)> {
    let outcome = thread.grading.as_ref()?;
    Some(failed(&Thread::primary(session).turn(), outcome, ModelUsage::default(), why))
}

/// The round's end when the grader stalled: it fails.
pub(super) fn no_verdict(primary: &Turn, outcome: &Outcome, usage: ModelUsage) -> (Event, OutcomeEvaluation) {
    failed(primary, outcome, usage, &format!("The grader ended {NUDGES} turns without a grade."))
}

fn failed(primary: &Turn, outcome: &Outcome, usage: ModelUsage, finding: &str) -> (Event, OutcomeEvaluation) {
    let grade =
        json!({"result": "failed", "criteria": [{"criterion": "grade", "met": false, "finding": finding}]});
    verdict(primary, outcome, &grade, usage)
}
