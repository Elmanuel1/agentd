//! The agent → client events.

use serde::{Deserialize, Serialize};
use shared::ContentBlock;

use super::EventId;
use crate::types::ThreadId;

/// The agent → client events (`type` `agent.*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentEvent {
    /// Agent text output.
    #[serde(rename = "agent.message")]
    Message {
        /// Content blocks.
        content: Vec<ContentBlock>,
    },
    /// Extended thinking.
    #[serde(rename = "agent.thinking")]
    Thinking {
        /// Summarized/omitted thinking text.
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    /// Built-in tool call.
    #[serde(rename = "agent.tool_use")]
    ToolUse {
        /// Tool name (bash/read/write/…).
        name: String,
        /// Tool input.
        input: serde_json::Value,
        /// `allow` (executed) or `ask` (awaiting confirmation).
        #[serde(skip_serializing_if = "Option::is_none")]
        evaluated_permission: Option<String>,
        /// How the permission was reached, e.g.
        /// `{"type":"auto","evaluated_permission":{"type":"allow"}}`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evaluation: Option<serde_json::Value>,
    },
    /// Built-in tool result.
    #[serde(rename = "agent.tool_result")]
    ToolResult {
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
    McpToolUse {
        /// Server name.
        mcp_server_name: String,
        /// Tool name.
        name: String,
        /// Input.
        input: serde_json::Value,
    },
    /// MCP tool result.
    #[serde(rename = "agent.mcp_tool_result")]
    McpToolResult {
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
    CustomToolUse {
        /// Tool name.
        name: String,
        /// Input.
        input: serde_json::Value,
    },
    /// A message this thread sent to another thread.
    #[serde(rename = "agent.thread_message_sent")]
    ThreadMessageSent {
        /// The message.
        content: Vec<ContentBlock>,
        /// The receiving agent, when the message starts or names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_agent_name: Option<String>,
        /// The receiving thread.
        to_session_thread_id: ThreadId,
    },
    /// A message this thread received from another thread.
    #[serde(rename = "agent.thread_message_received")]
    ThreadMessageReceived {
        /// The message.
        content: Vec<ContentBlock>,
        /// The sending agent.
        from_agent_name: String,
        /// The sending thread.
        from_session_thread_id: ThreadId,
    },
    /// History was compacted.
    #[serde(rename = "agent.thread_context_compacted")]
    ThreadContextCompacted {},
}
