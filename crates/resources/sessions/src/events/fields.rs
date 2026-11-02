use serde::{Deserialize, Serialize};

use super::EventId;

/// Why a session went idle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StopReason {
    /// Normal completion.
    EndTurn {},
    /// Blocked on a client action (tool confirmation / custom tool result).
    RequiresAction {
        /// The blocking event ids.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        event_ids: Vec<EventId>,
    },
    /// Terminal failure after retries.
    RetriesExhausted {},
}

impl StopReason {
    /// Terminal for the client's drain loop? (`requires_action` is NOT —
    /// the documented idle-gate rule.)
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::RequiresAction { .. })
    }
}

/// The rubric for an outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Rubric {
    /// Inline markdown.
    Text {
        /// The rubric content.
        content: String,
    },
    /// An uploaded file.
    File {
        /// `file_…`.
        file_id: String,
    },
}

/// Tool-confirmation verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationResult {
    /// Execute the tool.
    Allow,
    /// Refuse it.
    Deny,
}

/// Per-model-request usage on `span.model_request_end`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    /// Cache-creation tokens.
    pub cache_creation_input_tokens: u64,
    /// Cache-read tokens.
    pub cache_read_input_tokens: u64,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
}
