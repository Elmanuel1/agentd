//! The Anthropic Messages API client.

use async_trait::async_trait;
use serde_json::{json, Value};
use sessions::events::ModelUsage;

use super::{Block, Message, ModelClient, ModelError, ModelRequest, ModelResponse, Role, StopKind};

/// The Messages API version header every request sends.
const API_VERSION: &str = "2023-06-01";

/// A `ModelClient` over `POST /v1/messages`.
#[derive(Debug, Clone)]
pub struct AnthropicModel {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl AnthropicModel {
    /// Call `base_url` (normally `https://api.anthropic.com`) with `api_key`.
    pub fn new(http: reqwest::Client, api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self { http, api_key: api_key.into(), base_url: base_url.into() }
    }
}

/// The request body. The system prompt, the tool list and the newest message
/// carry cache breakpoints, so each step of a loop reads the conversation so
/// far from the prompt cache. The agent's effort is not sent: the Messages
/// API takes it only for some models, and refuses the request for the rest.
pub fn body(req: &ModelRequest) -> Value {
    let mut messages: Vec<Value> = req.messages.iter().map(message).collect();
    if let Some(last) = messages
        .last_mut()
        .and_then(|m| m.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|c| c.last_mut())
    {
        crate::json::set(last, "cache_control", json!({"type": "ephemeral"}));
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
        "metadata": {"user_id": req.agent},
    });
    // The API refuses an empty system text block, and a cache mark on one.
    if !req.system.is_empty() {
        let system = json!([{"type": "text", "text": req.system, "cache_control": {"type": "ephemeral"}}]);
        crate::json::set(&mut body, "system", system);
    }
    let mut tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
        .collect();
    if let Some(last) = tools.last_mut() {
        crate::json::set(last, "cache_control", json!({"type": "ephemeral"}));
    }
    if !tools.is_empty() {
        crate::json::set(&mut body, "tools", Value::Array(tools));
    }
    body
}

fn message(m: &Message) -> Value {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = m
        .content
        .iter()
        .map(|b| match b {
            Block::Text { text } => json!({"type": "text", "text": text}),
            Block::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            Block::ToolResult { tool_use_id, content, is_error } => json!({
                "type": "tool_result", "tool_use_id": tool_use_id,
                "content": [{"type": "text", "text": content}], "is_error": is_error,
            }),
        })
        .collect();
    json!({"role": role, "content": content})
}

/// Read a Messages API response.
pub fn parse(v: &Value) -> ModelResponse {
    let mut content = Vec::new();
    let mut thought = false;
    for block in crate::json::at(v, "/content").as_array().into_iter().flatten() {
        match crate::json::at(block, "/type").as_str() {
            Some("text") => {
                content.push(Block::text(crate::json::at(block, "/text").as_str().unwrap_or_default()))
            }
            Some("tool_use") => content.push(Block::ToolUse {
                id: crate::json::at(block, "/id").as_str().unwrap_or_default().to_owned(),
                name: crate::json::at(block, "/name").as_str().unwrap_or_default().to_owned(),
                input: crate::json::at(block, "/input").clone(),
            }),
            Some("thinking" | "redacted_thinking") => thought = true,
            _ => {}
        }
    }
    let stop = match crate::json::at(v, "/stop_reason").as_str() {
        Some("tool_use") => StopKind::ToolUse,
        Some("max_tokens") => StopKind::MaxTokens,
        _ => StopKind::EndTurn,
    };
    let u = &crate::json::at(v, "/usage");
    let usage = ModelUsage {
        cache_creation_input_tokens: crate::json::at(u, "/cache_creation_input_tokens").as_u64().unwrap_or(0),
        cache_read_input_tokens: crate::json::at(u, "/cache_read_input_tokens").as_u64().unwrap_or(0),
        input_tokens: crate::json::at(u, "/input_tokens").as_u64().unwrap_or(0),
        output_tokens: crate::json::at(u, "/output_tokens").as_u64().unwrap_or(0),
    };
    ModelResponse { content, stop, usage, thought, cost: None }
}

#[async_trait]
impl ModelClient for AnthropicModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url.trim_end_matches('/')))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .json(&body(&req))
            .send()
            .await
            .map_err(|e| ModelError::Retryable(e.to_string()))?;
        let status = resp.status().as_u16();
        let retry_after = ModelError::retry_after(resp.headers());
        let text = resp.text().await.map_err(|e| ModelError::Retryable(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(ModelError::from_status(status, text, retry_after));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| ModelError::Retryable(e.to_string()))?;
        Ok(parse(&v))
    }
}

#[cfg(test)]
mod tests;
