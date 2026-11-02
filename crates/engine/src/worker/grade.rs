//! One grading round: an independent grader, with its own context and the
//! read-only tools, grades the deliverable against the rubric and submits a
//! verdict. Only its `span.outcome_evaluation_*` events reach the session.

use serde_json::json;
use sessions::events::{ModelUsage, SpanEvent};
use sessions::types::{OutcomeEvaluation, OutcomeResult};
use shared::{ApiError, Usage};

use super::outcome::Outcome;
use super::SessionRun;
use crate::model::{Block, Message, ModelRequest, ModelTool, Role};
use crate::tools::{ToolCall, ToolExecutor};
use crate::turn::policy::Toolbox;

/// Most model calls one grading may make.
const GRADER_STEPS: usize = 80;
const VERDICT: &str = "submit_verdict";

const GRADER_SYSTEM: &str = "You are an outcome grader. You grade a deliverable against a rubric, independently: you did not produce it, and you check it against the source files in the sandbox, not against what such work usually says. Read the rubric in the message. Treat each `###` heading (or each numbered rule, when there are no headings) as one criterion. For each: reason, check the deliverable's values against the source material with the tools, and decide whether it passes. Change no file. When done, call `submit_verdict` once: `satisfied` only when every criterion passes, `needs_revision` when any fails, `failed` when the rubric does not apply to the deliverable. In `findings`, list every failed criterion with what is wrong, where in the deliverable, and the evidence that shows it.";

fn verdict_tool() -> ModelTool {
    ModelTool {
        name: VERDICT.into(),
        description: "Submit the grade.".into(),
        input_schema: json!({"type": "object", "properties": {
            "result": {"type": "string", "enum": ["satisfied", "needs_revision", "failed"]},
            "findings": {"type": "array", "items": {"type": "object", "properties": {
                "criterion": {"type": "string"}, "finding": {"type": "string"}}, "required": ["criterion", "finding"]}}},
            "required": ["result", "findings"]}),
    }
}

/// The explanation the agent and the client get, in live's wording.
fn explain(input: &serde_json::Value) -> (OutcomeResult, String) {
    let result = match crate::json::at(input, "/result").as_str() {
        Some("satisfied") => OutcomeResult::Satisfied,
        Some("failed") => OutcomeResult::Failed,
        _ => OutcomeResult::NeedsRevision,
    };
    let findings: Vec<String> = crate::json::at(input, "/findings")
        .as_array()
        .into_iter()
        .flatten()
        .map(|f| {
            format!(
                "- {}: {}",
                crate::json::at(f, "/criterion").as_str().unwrap_or(""),
                crate::json::at(f, "/finding").as_str().unwrap_or("")
            )
        })
        .collect();
    let text = match result {
        OutcomeResult::Satisfied => "An independent grader found every criterion met.".to_owned(),
        _ => format!(
            "An independent grader found the following criteria are not fully met:\n\n{}",
            findings.join("\n")
        ),
    };
    (result, text)
}

impl SessionRun {
    /// Grade one round of `outcome`; the verdict as recorded.
    pub(super) async fn grade(&self, outcome: &Outcome) -> Result<OutcomeResult, ApiError> {
        let start = self
            .emit(SpanEvent::OutcomeEvaluationStart {
                outcome_id: outcome.id.clone(),
                iteration: outcome.iteration,
            })
            .await?;
        let mut toolbox = Toolbox::read_only();
        toolbox.add(verdict_tool(), crate::turn::policy::Disposition::Allow { evaluation: None });
        let task = format!("The task the agent was given:\n\n{}\n\nThe rubric:\n\n{}\n\nGrade the deliverable as it is now in the sandbox.", outcome.description, outcome.rubric);
        let mut messages = vec![Message { role: Role::User, content: vec![Block::text(task)] }];
        let mut usage = ModelUsage::default();
        let mut verdict = None;
        let thread = format!("grader-{}-{}", outcome.id, outcome.iteration);
        for step in 0..GRADER_STEPS {
            self.emit(SpanEvent::OutcomeEvaluationOngoing { outcome_id: outcome.id.clone() }).await?;
            let req = ModelRequest {
                model: self.primary_model.clone(),
                agent: "grader".into(),
                system: GRADER_SYSTEM.into(),
                messages: messages.clone(),
                tools: toolbox.tools.clone(),
                max_tokens: self.engine.max_tokens,
                effort: None,
            };
            let response = match self.engine.models.complete(req).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = %e, "engine: grader model call failed");
                    break;
                }
            };
            usage.input_tokens += response.usage.input_tokens;
            usage.output_tokens += response.usage.output_tokens;
            usage.cache_read_input_tokens += response.usage.cache_read_input_tokens;
            usage.cache_creation_input_tokens += response.usage.cache_creation_input_tokens;
            let calls: Vec<(String, String, serde_json::Value)> =
                response.tool_uses().map(|(i, n, v)| (i.to_owned(), n.to_owned(), v.clone())).collect();
            messages.push(Message { role: Role::Assistant, content: response.content.clone() });
            if calls.is_empty() {
                if step + 1 < GRADER_STEPS {
                    messages.push(Message {
                        role: Role::User,
                        content: vec![Block::text(format!("Call `{VERDICT}` with your grade."))],
                    });
                }
                continue;
            }
            let mut results = Vec::new();
            for (id, name, input) in calls {
                if name == VERDICT {
                    verdict = Some(explain(&input));
                    results.push(Block::ToolResult {
                        tool_use_id: id,
                        content: "Recorded.".into(),
                        is_error: false,
                    });
                    continue;
                }
                let outcome = match toolbox.disposition(&name) {
                    Some(_) => {
                        self.tools
                            .execute(&ToolCall { id: id.clone(), name, input, thread: thread.clone() })
                            .await
                    }
                    None => crate::tools::ToolOutcome::err(format!("unknown tool: {name}")),
                };
                results.push(Block::ToolResult {
                    tool_use_id: id,
                    content: outcome.output,
                    is_error: outcome.is_error,
                });
            }
            messages.push(Message { role: Role::User, content: results });
            if verdict.is_some() {
                break;
            }
        }
        let (mut result, explanation) =
            verdict.unwrap_or((OutcomeResult::Failed, "The grader did not reach a verdict.".into()));
        if result == OutcomeResult::NeedsRevision && outcome.iteration + 1 >= outcome.max_iterations {
            result = OutcomeResult::MaxIterationsReached;
        }
        let total = Usage {
            cache_creation: None,
            cache_read_input_tokens: usage.cache_read_input_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        };
        self.engine.contract.add_usage(&self.ws, &self.session, &total).await?;
        self.emit(SpanEvent::OutcomeEvaluationEnd {
            outcome_evaluation_start_id: start.id,
            outcome_id: outcome.id.clone(),
            result,
            explanation: Some(explanation.clone()),
            iteration: outcome.iteration,
            usage,
        })
        .await?;
        let eval = OutcomeEvaluation {
            outcome_id: outcome.id.clone(),
            result,
            explanation: Some(explanation),
            iteration: outcome.iteration,
        };
        self.engine.contract.record_outcome(&self.ws, &self.session, eval).await?;
        Ok(result)
    }
}
