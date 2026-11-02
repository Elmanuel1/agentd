//! The ModelClient port — the LLM behind a trait.

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sessions::events::ModelUsage;

/// One message in the model conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMessage {
    /// `user` | `assistant`.
    pub role: String,
    /// Plain-text content (tool results serialized as text).
    pub content: String,
}

/// A tool the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelTool {
    /// Tool name.
    pub name: String,
    /// Description.
    pub description: String,
    /// JSON schema of the input.
    pub input_schema: serde_json::Value,
}

/// A completion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    /// Model id.
    pub model: String,
    /// System prompt.
    pub system: Option<String>,
    /// Conversation.
    pub messages: Vec<ModelMessage>,
    /// Available tools.
    pub tools: Vec<ModelTool>,
}

/// What the model produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModelOutput {
    /// Assistant text.
    Text {
        /// The text.
        text: String,
    },
    /// A tool call.
    ToolUse {
        /// Tool name.
        name: String,
        /// Tool input.
        input: serde_json::Value,
    },
}

/// A completion response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    /// Output blocks in order.
    pub outputs: Vec<ModelOutput>,
    /// Token usage.
    pub usage: ModelUsage,
}

impl ModelResponse {
    /// Whether the turn wants tool execution.
    pub fn has_tool_use(&self) -> bool {
        self.outputs.iter().any(|o| matches!(o, ModelOutput::ToolUse { .. }))
    }
}

/// Errors from the model backend.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ModelError {
    /// Transport/provider failure.
    #[error("model call failed: {0}")]
    Call(String),
    /// The scripted mock ran out of responses (test bug).
    #[error("mock exhausted")]
    Exhausted,
}

/// The LLM port.
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// One completion.
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError>;
}

/// Scripted model for tests: pops responses in order and records requests.
#[derive(Debug, Default)]
pub struct MockModel {
    script: Mutex<Vec<ModelResponse>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl MockModel {
    /// Build with a response script (played in order).
    pub fn scripted(responses: Vec<ModelResponse>) -> Self {
        let mut script = responses;
        script.reverse(); // pop() plays front-first
        Self { script: Mutex::new(script), requests: Mutex::new(Vec::new()) }
    }

    /// The requests the loop actually sent (assertable).
    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().clone()
    }
}

#[async_trait]
impl ModelClient for MockModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().push(req);
        self.script.lock().pop().ok_or(ModelError::Exhausted)
    }
}

/// Real Anthropic Messages-API client (feature `anthropic`).
#[cfg(feature = "anthropic")]
pub mod anthropic {
    use super::*;

    /// HTTP ModelClient over the Anthropic Messages API.
    #[derive(Debug)]
    pub struct AnthropicHttpModel {
        client: reqwest::Client,
        api_key: String,
        base_url: String,
    }

    impl AnthropicHttpModel {
        /// Build with an API key (base URL defaults to api.anthropic.com).
        pub fn new(api_key: impl Into<String>) -> Self {
            Self {
                client: reqwest::Client::new(),
                api_key: api_key.into(),
                base_url: "https://api.anthropic.com".into(),
            }
        }

        /// Override the base URL (self-tests, proxies).
        pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
            self.base_url = url.into();
            self
        }
    }

    #[async_trait]
    impl ModelClient for AnthropicHttpModel {
        async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
            let tools: Vec<serde_json::Value> = req
                .tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect();
            let mut body = serde_json::json!({
                "model": req.model,
                "max_tokens": 16000,
                "messages": req.messages.iter().map(|m| serde_json::json!({
                    "role": m.role, "content": m.content,
                })).collect::<Vec<_>>(),
            });
            if let Some(system) = &req.system {
                body["system"] = serde_json::json!(system);
            }
            if !tools.is_empty() {
                body["tools"] = serde_json::json!(tools);
            }
            let resp = self
                .client
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send()
                .await
                .map_err(|e| ModelError::Call(e.to_string()))?;
            let status = resp.status();
            let v: serde_json::Value = resp.json().await.map_err(|e| ModelError::Call(e.to_string()))?;
            if !status.is_success() {
                return Err(ModelError::Call(v.to_string()));
            }
            let mut outputs = Vec::new();
            for block in v["content"].as_array().unwrap_or(&Vec::new()) {
                match block["type"].as_str() {
                    Some("text") => outputs.push(ModelOutput::Text {
                        text: block["text"].as_str().unwrap_or_default().to_owned(),
                    }),
                    Some("tool_use") => outputs.push(ModelOutput::ToolUse {
                        name: block["name"].as_str().unwrap_or_default().to_owned(),
                        input: block["input"].clone(),
                    }),
                    _ => {}
                }
            }
            let usage = ModelUsage {
                input_tokens: v["usage"]["input_tokens"].as_u64().unwrap_or(0),
                output_tokens: v["usage"]["output_tokens"].as_u64().unwrap_or(0),
                cache_read_input_tokens: v["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0),
                cache_creation_input_tokens: 0,
            };
            Ok(ModelResponse { outputs, usage })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_plays_script_in_order_and_records_requests() {
        let mock = MockModel::scripted(vec![
            ModelResponse {
                outputs: vec![ModelOutput::Text { text: "one".into() }],
                usage: ModelUsage::default(),
            },
            ModelResponse {
                outputs: vec![ModelOutput::Text { text: "two".into() }],
                usage: ModelUsage::default(),
            },
        ]);
        let req =
            ModelRequest { model: "claude-sonnet-4-6".into(), system: None, messages: vec![], tools: vec![] };
        let a = mock.complete(req.clone()).await.unwrap();
        let b = mock.complete(req.clone()).await.unwrap();
        assert_eq!(a.outputs, vec![ModelOutput::Text { text: "one".into() }]);
        assert_eq!(b.outputs, vec![ModelOutput::Text { text: "two".into() }]);
        assert!(matches!(mock.complete(req).await, Err(ModelError::Exhausted)));
        assert_eq!(mock.requests().len(), 3);
    }
}
