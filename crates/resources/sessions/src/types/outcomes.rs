use serde::{Deserialize, Serialize};
use shared::{Id, Resource};

/// Marker for `outc_…` outcome ids.
#[derive(Debug, Clone, Copy)]
pub struct OutcomeResource_;
impl Resource for OutcomeResource_ {
    const PREFIX: &'static str = "outc";
    const LABEL: &'static str = "outcome";
}
/// `Id<OutcomeResource_>` alias.
pub type OutcomeId = Id<OutcomeResource_>;

/// Outcome evaluation record surfaced on the session object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutcomeEvaluation {
    /// The outcome.
    pub outcome_id: OutcomeId,
    /// Result of the latest grader iteration.
    pub result: OutcomeResult,
    /// Grader explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    /// 0-indexed iteration.
    pub iteration: u32,
}

/// Grader verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeResult {
    /// Rubric met — terminal.
    Satisfied,
    /// Iterate again.
    NeedsRevision,
    /// Iteration cap reached — terminal.
    MaxIterationsReached,
    /// Rubric fundamentally mismatched — terminal.
    Failed,
    /// Interrupted mid-grade — terminal.
    Interrupted,
}

impl OutcomeResult {
    /// Whether this verdict ends the outcome loop.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::NeedsRevision)
    }
}
