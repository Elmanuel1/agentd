//! The Anthropic Messages API client.

use serde_json::{Value, json};
use sessions::events::ModelUsage;

use super::{Block, Message, ModelError, ModelRequest, ModelResponse, NativeModelClient, Role, StopKind};

/// The Messages API version header every request sends.
const API_VERSION: &str = "2023-06-01";
/// The beta a compaction request, and every request carrying its block, sends.
const COMPACTION_BETA: &str = "compact-2026-09-04";

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

    /// A GET of `path` with the API's headers.
    pub(super) fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.base_url.trim_end_matches('/')))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
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
    if let Some(thinking) = thinking(&req.model, req.max_tokens) {
        crate::json::set(&mut body, "thinking", thinking);
    }
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
    if let Some(instructions) = &req.compact {
        crate::json::set(&mut body, "compaction", json!({"type": "summarize", "instructions": instructions}));
    }
    body
}

/// Whether `req` asks for a compaction or carries a compaction block.
fn compacts(req: &ModelRequest) -> bool {
    req.compact.is_some()
        || req.messages.iter().flat_map(|m| &m.content).any(|b| matches!(b, Block::Compaction { .. }))
}

/// The models that take a fixed thinking budget; every later one thinks adaptively.
const BUDGETED: [&str; 3] = ["claude-haiku-4-5", "claude-sonnet-4-5", "claude-opus-4-5"];

/// The request's `thinking`: adaptive, or for an older model a budget of a
/// quarter of the output cap, at least the API's minimum of 1024 tokens and
/// below the cap, so none when the cap leaves no room.
fn thinking(model: &str, max_tokens: u32) -> Option<Value> {
    let budget = (max_tokens / 4).max(1024);
    match BUDGETED.iter().any(|m| model.starts_with(m)) {
        true => (budget < max_tokens).then(|| json!({"type": "enabled", "budget_tokens": budget})),
        false => Some(json!({"type": "adaptive"})),
    }
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
            Block::Thinking { thinking, signature } => {
                json!({"type": "thinking", "thinking": thinking, "signature": signature})
            }
            Block::RedactedThinking { data } => json!({"type": "redacted_thinking", "data": data}),
            Block::Compaction { content, signature } => {
                json!({"type": "compaction", "content": content, "signature": signature})
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
            Some("compaction") => content.extend(serde_json::from_value::<Block>(block.clone()).ok()),
            Some("thinking" | "redacted_thinking") => {
                thought = true;
                content.extend(serde_json::from_value::<Block>(block.clone()).ok());
            }
            _ => {}
        }
    }
    let stop = match crate::json::at(v, "/stop_reason").as_str() {
        Some("tool_use") => StopKind::ToolUse,
        Some("max_tokens") => StopKind::MaxTokens,
        Some("compaction") => StopKind::Compaction,
        _ => StopKind::EndTurn,
    };
    let u = &crate::json::at(v, "/usage");
    let n = |u: &Value, f: &str| crate::json::at(u, f).as_u64().unwrap_or(0);
    let mut usage = ModelUsage {
        cache_creation_input_tokens: n(u, "/cache_creation_input_tokens"),
        cache_read_input_tokens: n(u, "/cache_read_input_tokens"),
        input_tokens: n(u, "/input_tokens"),
        output_tokens: n(u, "/output_tokens"),
    };
    // The top level leaves compaction out; the iterations' sum is the call's total.
    if let Some(its) = crate::json::at(u, "/iterations").as_array().filter(|a| !a.is_empty()) {
        usage.input_tokens = its.iter().map(|it| n(it, "/input_tokens")).sum();
        usage.output_tokens = its.iter().map(|it| n(it, "/output_tokens")).sum();
    }
    let cost = super::list_cost::of(crate::json::at(v, "/model").as_str().unwrap_or_default(), &usage);
    ModelResponse { content, stop, usage, thought, cost }
}

impl NativeModelClient for AnthropicModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        let resp = self
            .http
            .post(format!("{}/v1/messages", self.base_url.trim_end_matches('/')))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION);
        let resp = match compacts(&req) {
            true => resp.header("anthropic-beta", COMPACTION_BETA),
            false => resp,
        };
        let resp = resp.json(&body(&req)).send().await.map_err(|e| ModelError::Retryable(e.to_string()))?;
        let status = resp.status().as_u16();
        let retry_after = ModelError::retry_after(resp.headers());
        let text = resp.text().await.map_err(|e| ModelError::Retryable(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(ModelError::from_status(status, text, retry_after));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| ModelError::Retryable(e.to_string()))?;
        Ok(parse(&v))
    }

    async fn window(&self, model: &str) -> Result<u64, ModelError> {
        self.read_window(model).await
    }
}

#[cfg(test)]
mod tests;
