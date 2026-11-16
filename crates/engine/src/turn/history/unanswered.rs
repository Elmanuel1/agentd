//! A call the conversation ends on, and who answers it.

/// A tool call the client still has to answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Unanswered {
    /// The `agent.tool_use` / `agent.custom_tool_use` event id.
    pub id: String,
    /// The tool.
    pub name: String,
    /// Its input.
    pub input: serde_json::Value,
    /// A custom tool (the client runs it).
    pub custom: bool,
    /// A built-in or MCP call waiting on the client's confirmation.
    pub asked: bool,
    /// The MCP server whose tool it calls, which the loop calls itself.
    pub mcp: Option<String>,
}

impl Unanswered {
    /// The session's worker runs it: a built-in call the policy allowed.
    pub fn for_worker(&self) -> bool {
        !self.custom && !self.asked && self.mcp.is_none()
    }

    /// The loop calls it now: an MCP call the policy or the client allowed.
    pub fn for_loop(&self) -> bool {
        self.mcp.is_some() && !self.asked
    }
}
