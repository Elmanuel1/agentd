//! The session event protocol — one exhaustive enum for every wire event.
//!
//! Envelope (observed): every event carries `id` (`sevt_…`), `type`
//! (dot-namespaced), and `processed_at` (null until the agent consumes a
//! client-sent event). Payload fields are flattened beside the envelope.

use agentd_core::{ContentBlock, Id, Resource, Timestamp};
use serde::{Deserialize, Serialize};

use crate::types::OutcomeId;

/// Marker for `sevt_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct EventResource_;
impl Resource for EventResource_ {
    const PREFIX: &'static str = "sevt";
    const LABEL: &'static str = "event";
}
/// `Id<EventResource_>` alias.
pub type EventId = Id<EventResource_>;

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

/// Every event type on the wire — exhaustive by construction; adding a
/// variant forces every consumer match-site to update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    // ── client → session ──────────────────────────────────────────────────
    /// A user message.
    #[serde(rename = "user.message")]
    UserMessage {
        /// Content blocks.
        content: Vec<ContentBlock>,
    },
    /// Interrupt: jumps the queue, forces idle.
    #[serde(rename = "user.interrupt")]
    UserInterrupt {},
    /// Approve/deny a pending tool.
    #[serde(rename = "user.tool_confirmation")]
    UserToolConfirmation {
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
    UserCustomToolResult {
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
    UserDefineOutcome {
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

    // ── agent → client ────────────────────────────────────────────────────
    /// Agent text output.
    #[serde(rename = "agent.message")]
    AgentMessage {
        /// Content blocks.
        content: Vec<ContentBlock>,
    },
    /// Extended thinking.
    #[serde(rename = "agent.thinking")]
    AgentThinking {
        /// Summarized/omitted thinking text.
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    /// Built-in tool call.
    #[serde(rename = "agent.tool_use")]
    AgentToolUse {
        /// Tool name (bash/read/write/…).
        name: String,
        /// Tool input.
        input: serde_json::Value,
        /// `allow` (executed) or `ask` (awaiting confirmation).
        #[serde(skip_serializing_if = "Option::is_none")]
        evaluated_permission: Option<String>,
    },
    /// Built-in tool result.
    #[serde(rename = "agent.tool_result")]
    AgentToolResult {
        /// The tool_use event id.
        tool_use_id: EventId,
        /// Result content.
        content: Vec<ContentBlock>,
        /// Whether it errored.
        #[serde(default)]
        is_error: bool,
    },
    /// MCP tool call.
    #[serde(rename = "agent.mcp_tool_use")]
    AgentMcpToolUse {
        /// Server name.
        mcp_server_name: String,
        /// Tool name.
        name: String,
        /// Input.
        input: serde_json::Value,
    },
    /// MCP tool result.
    #[serde(rename = "agent.mcp_tool_result")]
    AgentMcpToolResult {
        /// The mcp_tool_use event id.
        mcp_tool_use_id: EventId,
        /// Content.
        content: Vec<ContentBlock>,
        /// Error flag.
        #[serde(default)]
        is_error: bool,
    },
    /// Custom tool call — session idles until the client answers.
    #[serde(rename = "agent.custom_tool_use")]
    AgentCustomToolUse {
        /// Tool name.
        name: String,
        /// Input.
        input: serde_json::Value,
    },
    /// History was compacted.
    #[serde(rename = "agent.thread_context_compacted")]
    AgentThreadContextCompacted {},

    // ── session status ────────────────────────────────────────────────────
    /// Awaiting input; inspect `stop_reason`.
    #[serde(rename = "session.status_idle")]
    SessionStatusIdle {
        /// Why.
        stop_reason: StopReason,
    },
    /// Actively working.
    #[serde(rename = "session.status_running")]
    SessionStatusRunning {},
    /// Rescheduling after a retryable error.
    #[serde(rename = "session.status_rescheduled")]
    SessionStatusRescheduled {},
    /// Terminal.
    #[serde(rename = "session.status_terminated")]
    SessionStatusTerminated {},
    /// An error surfaced mid-run.
    #[serde(rename = "session.error")]
    SessionError {
        /// Error detail.
        error: serde_json::Value,
    },

    // ── spans ─────────────────────────────────────────────────────────────
    /// Model inference started.
    #[serde(rename = "span.model_request_start")]
    SpanModelRequestStart {},
    /// Model inference finished.
    #[serde(rename = "span.model_request_end")]
    SpanModelRequestEnd {
        /// The matching start event.
        model_request_start_id: EventId,
        /// Whether it errored.
        #[serde(default)]
        is_error: bool,
        /// Usage.
        model_usage: ModelUsage,
    },
    /// Grader began an iteration.
    #[serde(rename = "span.outcome_evaluation_start")]
    SpanOutcomeEvaluationStart {
        /// Outcome.
        outcome_id: OutcomeId,
        /// 0-indexed iteration.
        iteration: u32,
    },
    /// Grader heartbeat.
    #[serde(rename = "span.outcome_evaluation_ongoing")]
    SpanOutcomeEvaluationOngoing {
        /// Outcome.
        outcome_id: OutcomeId,
    },
    /// Grader verdict for one iteration.
    #[serde(rename = "span.outcome_evaluation_end")]
    SpanOutcomeEvaluationEnd {
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

impl EventPayload {
    /// Is this a client-sent event (subject to `processed_at` stamping)?
    pub fn is_user_event(&self) -> bool {
        matches!(
            self,
            Self::UserMessage { .. }
                | Self::UserInterrupt {}
                | Self::UserToolConfirmation { .. }
                | Self::UserCustomToolResult { .. }
                | Self::UserDefineOutcome { .. }
                | Self::SystemMessage { .. }
        )
    }
}

/// The full wire event: envelope + flattened payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// `sevt_…`.
    pub id: EventId,
    /// Set when the agent consumes the event. Live OMITS this field until the
    /// event is processed (client-sent events appear twice on the stream:
    /// queued without it, then processed with it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<Timestamp>,
    /// The typed payload, flattened beside the envelope.
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl Event {
    /// Wrap a payload with a fresh id and no processed stamp.
    pub fn new(payload: EventPayload) -> Self {
        Self { id: EventId::generate(), processed_at: None, payload }
    }
}

#[cfg(test)]
mod tests;
