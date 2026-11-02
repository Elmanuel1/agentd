//! The agent → client events.

use serde::{Deserialize, Serialize};
use shared::ContentBlock;

use super::EventId;

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
    /// History was compacted.
    #[serde(rename = "agent.thread_context_compacted")]
    ThreadContextCompacted {},
}
