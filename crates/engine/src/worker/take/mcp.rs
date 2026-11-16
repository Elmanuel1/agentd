//! How the client's answers reach an MCP call: its result is an
//! `agent.mcp_tool_result`, and the loop, not the worker, runs it.

use sessions::events::{AgentEvent, Event, EventId, EventPayload};
use shared::ContentBlock;

use super::super::turn::Turn;

/// Whether `id` is an `agent.mcp_tool_use` in `log`.
pub(super) fn is_mcp(log: &[Event], id: &EventId) -> bool {
    log.iter().any(|e| e.id == *id && matches!(e.payload, EventPayload::Agent(AgentEvent::McpToolUse { .. })))
}

/// The error result of call `id`, as its kind of call records one.
pub(super) fn refusal(log: &[Event], turn: &Turn, id: &EventId, text: String) -> Event {
    let content = vec![ContentBlock::text(text)];
    match is_mcp(log, id) {
        true => {
            turn.event(AgentEvent::McpToolResult { mcp_tool_use_id: id.clone(), content, is_error: true })
        }
        false => turn.event(AgentEvent::ToolResult { tool_use_id: id.clone(), content, is_error: true }),
    }
}

/// The call a result answers.
pub(super) fn answered(e: &Event) -> Option<&EventId> {
    match &e.payload {
        EventPayload::Agent(AgentEvent::ToolResult { tool_use_id, .. })
        | EventPayload::Agent(AgentEvent::McpToolResult { mcp_tool_use_id: tool_use_id, .. }) => {
            Some(tool_use_id)
        }
        _ => None,
    }
}
