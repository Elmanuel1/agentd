//! The ModelClient port: the LLM behind a trait, with a provider-neutral
//! conversation. Each client maps these types to its own wire format.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sessions::events::ModelUsage;

pub mod anthropic;
pub mod mock;
pub mod openai;
pub mod router;

pub use mock::MockModel;
pub use router::{ModelRouter, ProviderConfig};

/// Who said a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The user side: messages, tool results, delegated replies.
    User,
    /// The model.
    Assistant,
}

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
    /// The result of a tool call.
    ToolResult {
        /// The call it answers.
        tool_use_id: String,
        /// The result text.
        content: String,
        /// Whether the tool failed.
        is_error: bool,
    },
}

impl Block {
    /// A text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// One message in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Who said it.
    pub role: Role,
    /// Its blocks, in order.
    pub content: Vec<Block>,
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelTool {
    /// Tool name.
    pub name: String,
    /// What it does, for the model.
    pub description: String,
    /// JSON schema of the input.
    pub input_schema: serde_json::Value,
}

/// A completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Model id, as the agent names it.
    pub model: String,
    /// The agent's name; the scripted mock plays each agent its own script.
    pub agent: String,
    /// System prompt.
    pub system: String,
    /// Conversation.
    pub messages: Vec<Message>,
    /// Available tools.
    pub tools: Vec<ModelTool>,
    /// The output cap.
    pub max_tokens: u32,
    /// Reasoning effort (`low` … `max`), when the agent sets one.
    pub effort: Option<String>,
}

/// Why the model stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopKind {
    /// It finished its turn.
    EndTurn,
    /// It wants its tool calls run.
    ToolUse,
    /// It hit the output cap.
    MaxTokens,
}

/// A completion response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Text and tool calls, in order.
    pub content: Vec<Block>,
    /// Why it stopped.
    pub stop: StopKind,
    /// Token usage.
    pub usage: ModelUsage,
    /// The model reasoned before answering (reported as `agent.thinking`).
    #[serde(default)]
    pub thought: bool,
}

impl ModelResponse {
    /// A response that only says `text`.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Block::text(text)],
            stop: StopKind::EndTurn,
            usage: ModelUsage::default(),
            thought: false,
        }
    }

    /// The tool calls it asks for.
    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &serde_json::Value)> {
        self.content.iter().filter_map(|b| match b {
            Block::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

/// Errors from the model backend.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// Worth retrying: rate limits, overload, 5xx, transport.
    #[error("model call failed (retryable): {0}")]
    Retryable(String),
    /// Not worth retrying: bad request, auth, unknown model.
    #[error("model call failed: {0}")]
    Fatal(String),
}

impl ModelError {
    /// Classify an HTTP status from a provider.
    pub fn from_status(status: u16, body: String) -> Self {
        if status == 408 || status == 409 || status == 429 || status >= 500 {
            Self::Retryable(format!("{status}: {body}"))
        } else {
            Self::Fatal(format!("{status}: {body}"))
        }
    }
}

/// The LLM port.
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// One completion.
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError>;
}
