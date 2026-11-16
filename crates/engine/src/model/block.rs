//! The blocks of a message.

use serde::{Deserialize, Serialize};

/// One block of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// Text.
    Text {
        /// The text.
        text: String,
    },
    /// A tool call by the model.
    ToolUse {
        /// The call's id, echoed by its result.
        id: String,
        /// The tool.
        name: String,
        /// Its input.
        input: serde_json::Value,
    },
    /// The model's reasoning, sent back unmodified in the turn that follows.
    Thinking {
        /// The thinking text (empty when omitted).
        thinking: String,
        /// Its signature.
        signature: String,
    },
    /// Reasoning the API returns encrypted, sent back unmodified.
    RedactedThinking {
        /// The encrypted reasoning.
        data: String,
    },
    /// The result of a tool call.
    ToolResult {
        /// The call it answers.
        tool_use_id: String,
        /// The result text.
        content: String,
        /// Whether the tool failed.
        is_error: bool,
    },
    /// A compaction's summary, signed by the Messages API, sent back
    /// unmodified first in every later request (C1).
    Compaction {
        /// The summary.
        content: String,
        /// Its signature.
        signature: String,
    },
}

impl Block {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}
