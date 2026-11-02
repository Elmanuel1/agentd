//! Outcomes: after the agent's work, an independent grader (its own
//! context, read-only tools in the same sandbox) grades the deliverable
//! against the rubric. Only the `span.outcome_evaluation_*` events of the
//! grader reach the session; its findings go back to the agent.

use sessions::events::{Event, EventPayload, SpanEvent, UserEvent};
use sessions::types::OutcomeId;

/// Rounds when `max_iterations` is left out (documented default).
pub const DEFAULT_MAX_ITERATIONS: u32 = 3;
/// The outcome the session is working toward.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Its id.
    pub id: OutcomeId,
    /// The task.
    pub description: String,
    /// The rubric text.
    pub rubric: String,
    /// Rounds allowed.
    pub max_iterations: u32,
    /// Rounds graded so far.
    pub iteration: u32,
}

/// The active outcome in `events`: the last one defined and processed that
/// has no final verdict yet.
pub fn active(
    events: &[Event],
    rubric_text: impl Fn(&sessions::events::Rubric) -> String,
) -> Option<Outcome> {
    let mut current: Option<Outcome> = None;
    for e in events {
        match &e.payload {
            EventPayload::User(UserEvent::DefineOutcome {
                description,
                rubric,
                max_iterations,
                outcome_id: Some(id),
            }) if e.processed_at.is_some() => {
                current = Some(Outcome {
                    id: id.clone(),
                    description: description.clone(),
                    rubric: rubric_text(rubric),
                    max_iterations: max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS),
                    iteration: 0,
                });
            }
            EventPayload::Span(SpanEvent::OutcomeEvaluationEnd { outcome_id, result, iteration, .. }) => {
                if current.as_ref().is_some_and(|o| &o.id == outcome_id) {
                    if result.is_terminal() {
                        current = None;
                    } else if let Some(o) = current.as_mut() {
                        o.iteration = iteration + 1;
                    }
                }
            }
            _ => {}
        }
    }
    current
}
