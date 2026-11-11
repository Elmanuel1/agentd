//! The ModelClient port: the LLM behind a trait, with a provider-neutral
//! conversation. Each client maps these types to its own wire format.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sessions::events::ModelUsage;

pub mod anthropic;
pub mod budget;
pub mod openai;
pub mod retry;
pub mod router;

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
    /// The agent's name, sent as the request's user; the scripted model
    /// plays each agent its own script.
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
    /// The request's list price in USD, where the provider reports one.
    #[serde(default)]
    pub cost: Option<f64>,
}

impl ModelResponse {
    /// A response that only says `text`.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Block::text(text)],
            stop: StopKind::EndTurn,
            usage: ModelUsage::default(),
            thought: false,
            cost: None,
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
    /// Worth retrying, no sooner than the provider's `retry-after`.
    #[error("model call failed (retry after {1:?}): {0}")]
    RetryAfter(String, std::time::Duration),
    /// Not worth retrying: bad request, auth, unknown model.
    #[error("model call failed: {0}")]
    Fatal(String),
}

impl ModelError {
    /// Classify an HTTP status from a provider, with its `retry-after`
    /// seconds when it sent them.
    pub fn from_status(status: u16, body: String, retry_after: Option<std::time::Duration>) -> Self {
        let reason = format!("{status}: {body}");
        let retryable = status == 408 || status == 409 || status == 429 || status >= 500;
        match (retryable, retry_after) {
            (false, _) => Self::Fatal(reason),
            (true, Some(after)) => Self::RetryAfter(reason, after),
            (true, None) => Self::Retryable(reason),
        }
    }

    /// Read `retry-after` as seconds.
    pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<std::time::Duration> {
        let secs = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse::<f64>().ok()?;
        (secs.is_finite() && secs >= 0.0).then(|| std::time::Duration::from_secs_f64(secs))
    }
}

/// The LLM port.
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// One completion.
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError>;
}
