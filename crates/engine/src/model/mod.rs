//! The ModelClient port: the LLM behind a trait, with a provider-neutral
//! conversation. Each client maps these types to its own wire format.

use serde::{Deserialize, Serialize};
use sessions::events::ModelUsage;
use shared::ports::BoxFuture;
use std::future::Future;

pub mod anthropic;
mod block;
pub mod budget;
pub mod openai;
pub mod retry;
pub mod router;
mod windows;

pub use block::Block;
pub use router::{ModelRouter, Provider as ModelProvider, ProviderConfig};

/// Who said a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The user side: messages, tool results, delegated replies.
    User,
    /// The model.
    Assistant,
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
    /// Summarise the conversation instead of answering, with these
    /// instructions (CP-7, CP-8).
    pub compact: Option<String>,
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
    /// It summarised the conversation into a compaction block.
    Compaction,
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
pub trait ModelClient: Send + Sync {
    /// One completion.
    fn complete(&self, req: ModelRequest) -> BoxFuture<'_, Result<ModelResponse, ModelError>>;
    /// The model's context window in tokens, from its provider's model list.
    fn window<'a>(&'a self, model: &'a str) -> BoxFuture<'a, Result<u64, ModelError>>;
}

/// What an adapter implements to provide [`ModelClient`]; the methods are documented there.
pub trait ModelClientImpl: Send + Sync {
    fn complete(&self, req: ModelRequest) -> impl Future<Output = Result<ModelResponse, ModelError>> + Send;
    fn window(&self, model: &str) -> impl Future<Output = Result<u64, ModelError>> + Send;
}

impl<T: ModelClientImpl> ModelClient for T {
    fn complete(&self, req: ModelRequest) -> BoxFuture<'_, Result<ModelResponse, ModelError>> {
        Box::pin(ModelClientImpl::complete(self, req))
    }
    fn window<'a>(&'a self, model: &'a str) -> BoxFuture<'a, Result<u64, ModelError>> {
        Box::pin(ModelClientImpl::window(self, model))
    }
}
