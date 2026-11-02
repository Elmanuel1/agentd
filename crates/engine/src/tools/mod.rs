//! The agent toolset (`agent_toolset_20260401`): every tool runs inside the
//! session's sandbox through the [`shared::ports::sandbox::Sandbox`] port,
//! except the web tools, which agentd runs itself.

use async_trait::async_trait;

mod files;
pub mod sandbox;
pub mod specs;
pub mod text;
pub mod web;

pub use sandbox::SandboxTools;

/// One tool call, as the loop hands it to an executor.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The `agent.tool_use` event id.
    pub id: String,
    /// The tool.
    pub name: String,
    /// Its input.
    pub input: serde_json::Value,
    /// The session thread making the call; each thread has its own shell.
    pub thread: String,
}

/// What a tool returned, for the model and for `agent.tool_result`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The result text.
    pub output: String,
    /// Whether the tool failed.
    pub is_error: bool,
}

impl ToolOutcome {
    /// A successful result.
    pub fn ok(s: impl Into<String>) -> Self {
        Self { output: s.into(), is_error: false }
    }

    /// A failed result.
    pub fn err(s: impl Into<String>) -> Self {
        Self { output: s.into(), is_error: true }
    }
}

/// Runs the built-in tools for one session.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Run one call. Failures come back as an `is_error` outcome for the
    /// model, never as an error of the loop.
    async fn execute(&self, call: &ToolCall) -> ToolOutcome;
}
