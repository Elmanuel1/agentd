//! The client → session events.

use serde::{Deserialize, Serialize};
use shared::ContentBlock;

use super::{ConfirmationResult, EventId, Rubric};
use crate::types::OutcomeId;

/// The client → session events (`type` `user.*` and `system.message`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserEvent {
    /// A user message.
    #[serde(rename = "user.message")]
    Message {
        /// Content blocks.
        content: Vec<ContentBlock>,
    },
    /// Interrupt: jumps the queue, forces idle.
    #[serde(rename = "user.interrupt")]
    Interrupt {},
    /// Approve/deny a pending tool.
    #[serde(rename = "user.tool_confirmation")]
    ToolConfirmation {
        /// The `agent.tool_use` event id being answered.
        tool_use_id: EventId,
        /// allow | deny.
        result: ConfirmationResult,
        /// Optional deny explanation, surfaced to the model.
        #[serde(skip_serializing_if = "Option::is_none")]
        deny_message: Option<String>,
    },
    /// Result for a custom (client-executed) tool.
    #[serde(rename = "user.custom_tool_result")]
    CustomToolResult {
        /// The `agent.custom_tool_use` event id.
        custom_tool_use_id: EventId,
        /// Result content.
        content: Vec<ContentBlock>,
        /// Whether the tool failed.
        #[serde(default)]
        is_error: bool,
    },
    /// Start a rubric-graded outcome loop.
    #[serde(rename = "user.define_outcome")]
    DefineOutcome {
        /// The task.
        description: String,
        /// The rubric.
        rubric: Rubric,
        /// 1–20, default 3.
        #[serde(skip_serializing_if = "Option::is_none")]
        max_iterations: Option<u32>,
        /// Server-assigned on echo.
        #[serde(skip_serializing_if = "Option::is_none")]
        outcome_id: Option<OutcomeId>,
    },
    /// Mid-session system prompt update.
    #[serde(rename = "system.message")]
    SystemMessage {
        /// Content blocks (1–1000 text items).
        content: Vec<ContentBlock>,
    },
}
