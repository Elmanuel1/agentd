//! The span events.

use serde::{Deserialize, Serialize};

use super::{EventId, ModelUsage, SpanModelUsage};
use crate::types::OutcomeId;

/// The span events (`type` `span.*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SpanEvent {
    /// Model inference started.
    #[serde(rename = "span.model_request_start")]
    ModelRequestStart {},
    /// Model inference finished.
    #[serde(rename = "span.model_request_end")]
    ModelRequestEnd {
        /// The matching start event.
        model_request_start_id: EventId,
        /// Whether it errored.
        #[serde(default)]
        is_error: bool,
        /// Usage.
        model_usage: SpanModelUsage,
    },
    /// Grader began an iteration.
    #[serde(rename = "span.outcome_evaluation_start")]
    OutcomeEvaluationStart {
        /// Outcome.
        outcome_id: OutcomeId,
        /// 0-indexed iteration.
        iteration: u32,
    },
    /// Grader heartbeat.
    #[serde(rename = "span.outcome_evaluation_ongoing")]
    OutcomeEvaluationOngoing {
        /// Outcome.
        outcome_id: OutcomeId,
    },
    /// Grader verdict for one iteration.
    #[serde(rename = "span.outcome_evaluation_end")]
    OutcomeEvaluationEnd {
        /// The matching start event.
        outcome_evaluation_start_id: EventId,
        /// Outcome.
        outcome_id: OutcomeId,
        /// Verdict.
        result: crate::types::OutcomeResult,
        /// Explanation.
        #[serde(skip_serializing_if = "Option::is_none")]
        explanation: Option<String>,
        /// Iteration.
        iteration: u32,
        /// Grader usage.
        usage: ModelUsage,
    },
}
