//! The outcome loop: iterate → grade → revise, driven by a rubric.

use std::sync::Arc;

use async_trait::async_trait;
use sessions::events::{EventPayload, ModelUsage, Rubric};
use sessions::types::{OutcomeEvaluation, OutcomeId, OutcomeResult, SessionId};
use sessions::EngineContract;
use shared::{ApiError, WorkspaceId};

use crate::turn::{TurnEnd, TurnRunner};

/// The grader port — an independent context that scores the work against
/// the rubric. LLM implementation later; deterministic fakes in tests.
#[async_trait]
pub trait Grader: Send + Sync {
    /// Grade one iteration.
    async fn grade(
        &self,
        description: &str,
        rubric: &Rubric,
        iteration: u32,
    ) -> Result<GradeVerdict, ApiError>;
}

/// A grader verdict.
#[derive(Debug, Clone)]
pub struct GradeVerdict {
    /// satisfied / needs_revision / failed.
    pub result: OutcomeResult,
    /// Explanation.
    pub explanation: String,
    /// Grader token usage.
    pub usage: ModelUsage,
}

/// Default `max_iterations` when the client omits it (documented).
pub const DEFAULT_MAX_ITERATIONS: u32 = 3;

/// Runs the outcome loop for one `user.define_outcome`.
pub struct OutcomeLoop {
    contract: Arc<dyn EngineContract>,
    grader: Arc<dyn Grader>,
    ws: WorkspaceId,
    session_id: SessionId,
}

impl std::fmt::Debug for OutcomeLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutcomeLoop").finish_non_exhaustive()
    }
}

impl OutcomeLoop {
    /// Build the loop.
    pub fn new(
        contract: Arc<dyn EngineContract>,
        grader: Arc<dyn Grader>,
        ws: WorkspaceId,
        session_id: SessionId,
    ) -> Self {
        Self { contract, grader, ws, session_id }
    }

    /// Drive iterate→grade→revise until terminal. The runner executes the
    /// work; the grader judges each iteration in its own context.
    pub async fn run(
        &self,
        runner: &mut TurnRunner,
        outcome_id: OutcomeId,
        description: &str,
        rubric: &Rubric,
        max_iterations: Option<u32>,
    ) -> Result<OutcomeResult, ApiError> {
        let cap = max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS);
        let mut iteration: u32 = 0;
        loop {
            // 1. Work.
            let end = runner.run_turn().await?;
            if end == TurnEnd::Interrupted {
                let eval = OutcomeEvaluation {
                    outcome_id: outcome_id.clone(),
                    result: OutcomeResult::Interrupted,
                    explanation: None,
                    iteration,
                };
                self.contract.record_outcome(&self.ws, &self.session_id, eval).await?;
                return Ok(OutcomeResult::Interrupted);
            }

            // 2. Grade (with span events).
            let start = self
                .contract
                .emit(
                    &self.ws,
                    &self.session_id,
                    EventPayload::SpanOutcomeEvaluationStart { outcome_id: outcome_id.clone(), iteration },
                )
                .await?;
            let verdict = self.grader.grade(description, rubric, iteration).await?;
            // needs_revision at the cap becomes max_iterations_reached.
            let result = if verdict.result == OutcomeResult::NeedsRevision && iteration + 1 >= cap {
                OutcomeResult::MaxIterationsReached
            } else {
                verdict.result
            };
            self.contract
                .emit(
                    &self.ws,
                    &self.session_id,
                    EventPayload::SpanOutcomeEvaluationEnd {
                        outcome_evaluation_start_id: start.id,
                        outcome_id: outcome_id.clone(),
                        result,
                        explanation: Some(verdict.explanation.clone()),
                        iteration,
                        usage: verdict.usage,
                    },
                )
                .await?;
            self.contract
                .record_outcome(
                    &self.ws,
                    &self.session_id,
                    OutcomeEvaluation {
                        outcome_id: outcome_id.clone(),
                        result,
                        explanation: Some(verdict.explanation),
                        iteration,
                    },
                )
                .await?;

            if result.is_terminal() {
                return Ok(result);
            }
            iteration += 1;
        }
    }
}
