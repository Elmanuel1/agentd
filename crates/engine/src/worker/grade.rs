//! Outcomes (L-5): when the primary thread ends a turn on an outcome not
//! yet met, an independent grader grades the work on a thread of its own,
//! one model call a step, between `span.outcome_evaluation_start` and
//! `_end`; its built-in calls go to the session's worker without idling the
//! session. The primary thread then takes one more turn on the verdict.

use std::collections::HashMap;

use agents::AgentSnapshot;
use serde_json::Value;
use sessions::events::{Event, EventId, EventPayload, ModelUsage, Rubric, SpanEvent, UserEvent};
use sessions::types::{OutcomeEvaluation, OutcomeId, OutcomeResult, Session, ThreadId};

use super::team::Thread;
use super::turn::Turn;
use crate::model::{Block, Message, Role};
use crate::turn::history::History;

/// Rounds when `max_iterations` is left out (documented default).
const DEFAULT_MAX_ITERATIONS: u32 = 3;
/// After a grader reply with no grade.
const GRADE_NUDGE: &str = "Reply with only your grade, as the JSON object your instructions describe.";

const GRADER_SYSTEM: &str = "You are an outcome grader. You grade a deliverable against a rubric, independently: you did not produce it, and you check it against the source files in the sandbox, not against what such work usually says. Read the rubric in the message. Treat each `###` heading (or each numbered rule, when there are no headings) as one criterion. For each: reason, check the deliverable's values against the source material with the tools, and decide whether it is met. Change no file. When done, reply with only a JSON object: {\"result\": \"satisfied\" when every criterion is met, \"needs_revision\" when any is not, or \"failed\" when the rubric does not apply to the deliverable, \"criteria\": [{\"criterion\": its heading, \"met\": true or false, \"finding\": what you found, where in the deliverable, and the evidence}]}, listing every criterion.";

/// The outcome the session works toward, and its open grading round.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Outcome {
    pub(super) id: OutcomeId,
    description: String,
    rubric: Rubric,
    max_iterations: u32,
    pub(super) iteration: u32,
    /// The open round's `span.outcome_evaluation_start`.
    pub(super) open: Option<EventId>,
}

impl Outcome {
    /// The grader's thread for the open round, named after its start.
    pub(super) fn thread(&self, session: &Session) -> Option<Thread> {
        let start = self.open.as_ref()?;
        let body = start.as_str().strip_prefix("sevt_").unwrap_or(start.as_str());
        let id = ThreadId::parse(&format!("sthr_{body}")).ok()?;
        let mut agent: AgentSnapshot = session.agent.clone();
        agent.config.name = "grader".into();
        agent.config.system = Some(GRADER_SYSTEM.into());
        agent.config.multiagent = None;
        Some(Thread { id, agent, primary: false, grading: Some(self.clone()) })
    }
}

/// The outcome in `log` with no final verdict yet: the last one defined.
pub(super) fn active(log: &[Event]) -> Option<Outcome> {
    let mut current: Option<Outcome> = None;
    for e in log.iter().filter(|e| e.processed_at.is_some()) {
        match &e.payload {
            EventPayload::User(UserEvent::DefineOutcome {
                description,
                rubric,
                max_iterations,
                outcome_id: Some(id),
            }) => {
                let rubric = rubric.clone();
                let max_iterations = max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS);
                current = Some(Outcome {
                    id: id.clone(),
                    description: description.clone(),
                    rubric,
                    max_iterations,
                    iteration: 0,
                    open: None,
                });
            }
            EventPayload::Span(SpanEvent::OutcomeEvaluationStart { outcome_id, .. }) => {
                current
                    .as_mut()
                    .filter(|o| &o.id == outcome_id)
                    .into_iter()
                    .for_each(|o| o.open = Some(e.id.clone()));
            }
            EventPayload::Span(SpanEvent::OutcomeEvaluationEnd { outcome_id, result, iteration, .. }) => {
                let ours = current.as_ref().is_some_and(|o| &o.id == outcome_id);
                if ours && result.is_terminal() {
                    current = None;
                }
                current.as_mut().filter(|o| &o.id == outcome_id).into_iter().for_each(|o| {
                    o.iteration = iteration + 1;
                    o.open = None;
                });
            }
            _ => {}
        }
    }
    current
}

/// The round's start, written as the primary thread's turn ends.
pub(super) fn begin(primary: &Turn, outcome: &Outcome) -> Event {
    primary.event(SpanEvent::OutcomeEvaluationStart {
        outcome_id: outcome.id.clone(),
        iteration: outcome.iteration,
    })
}

/// The grader's conversation: the task and rubric, then its own steps, with
/// a nudge after a step that called nothing.
pub(super) fn conversation(
    outcome: &Outcome,
    rubrics: &HashMap<String, String>,
    mut history: History,
) -> History {
    let rubric = match &outcome.rubric {
        Rubric::Text { content } => content.as_str(),
        Rubric::File { file_id } => rubrics.get(file_id).map(String::as_str).unwrap_or_default(),
    };
    let task = format!(
        "The task the agent was given:\n\n{}\n\nThe rubric:\n\n{rubric}\n\nGrade the deliverable as it is now in the sandbox.",
        outcome.description
    );
    history.messages.insert(0, Message { role: Role::User, content: vec![Block::text(task)] });
    if history.messages.last().is_some_and(|m| m.role == Role::Assistant) && history.unanswered.is_empty() {
        history.messages.push(Message { role: Role::User, content: vec![Block::text(GRADE_NUDGE)] });
    }
    history
}

/// The grade in a grader's reply `text`: its JSON object with a `result`.
pub(super) fn graded(text: &str) -> Option<Value> {
    let (start, end) = (text.find('{')?, text.rfind('}')?);
    let grade: Value = serde_json::from_str(text.get(start..=end)?).ok()?;
    grade.get("result").is_some_and(Value::is_string).then_some(grade)
}

/// The round's end for `grade`, as the primary thread's event, and the
/// evaluation the session records. The explanation is worded as live's.
pub(super) fn verdict(
    primary: &Turn,
    outcome: &Outcome,
    grade: &Value,
    usage: ModelUsage,
) -> (Event, OutcomeEvaluation) {
    let mut result = match grade.get("result").and_then(Value::as_str) {
        Some("satisfied") => OutcomeResult::Satisfied,
        Some("failed") => OutcomeResult::Failed,
        _ => OutcomeResult::NeedsRevision,
    };
    if result == OutcomeResult::NeedsRevision && outcome.iteration + 1 >= outcome.max_iterations {
        result = OutcomeResult::MaxIterationsReached;
    }
    let criteria = grade.get("criteria").and_then(Value::as_array).cloned().unwrap_or_default();
    let line = |c: &Value| {
        let met = match c["met"].as_bool() {
            Some(true) => "met",
            _ => "not met",
        };
        format!(
            "- {} ({met}): {}",
            c["criterion"].as_str().unwrap_or(""),
            c["finding"].as_str().unwrap_or("")
        )
    };
    let explanation = match result {
        OutcomeResult::Satisfied => {
            let lines: Vec<String> = criteria.iter().map(line).collect();
            format!("An independent grader found all criteria met:\n\n{}", lines.join("\n"))
        }
        _ => {
            let unmet = criteria.iter().filter(|c| c["met"].as_bool() != Some(true));
            let lines: Vec<String> = unmet.map(line).collect();
            format!(
                "An independent grader found the following criteria are not fully met:\n\n{}",
                lines.join("\n")
            )
        }
    };
    let end = primary.event(SpanEvent::OutcomeEvaluationEnd {
        outcome_evaluation_start_id: outcome.open.clone().unwrap_or_else(EventId::generate),
        outcome_id: outcome.id.clone(),
        result,
        explanation: Some(explanation.clone()),
        iteration: outcome.iteration,
        usage,
    });
    let eval = OutcomeEvaluation {
        outcome_id: outcome.id.clone(),
        result,
        explanation: Some(explanation),
        iteration: outcome.iteration,
    };
    (end, eval)
}
