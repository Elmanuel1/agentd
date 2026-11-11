//! An OpenAI-compatible chat completions client (OpenRouter and the like).

use async_trait::async_trait;
use serde_json::{json, Value};
use sessions::events::ModelUsage;

use super::{Block, Message, ModelClient, ModelError, ModelRequest, ModelResponse, Role, StopKind};

/// A `ModelClient` over `POST {base_url}/chat/completions`.
#[derive(Debug, Clone)]
pub struct OpenAiCompatModel {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl OpenAiCompatModel {
    /// Call `base_url` (e.g. `https://openrouter.ai/api/v1`) with `api_key`.
    pub fn new(http: reqwest::Client, api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self { http, api_key: api_key.into(), base_url: base_url.into() }
    }
}

/// The request body. Tool results become `tool` messages; text sent beside
/// them follows as a user message.
pub fn body(req: &ModelRequest) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    for m in &req.messages {
        push_message(&mut messages, m);
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": messages,
        "usage": {"include": true},
        "user": req.agent,
    });
    if !req.tools.is_empty() {
        crate::json::set(
            &mut body,
            "tools",
            req.tools
                .iter()
                .map(|t| {
                    json!({"type": "function", "function": {
                        "name": t.name, "description": t.description, "parameters": t.input_schema,
                    }})
                })
                .collect(),
        );
    }
    if let Some(effort) = &req.effort {
        let effort = match effort.as_str() {
            "xhigh" | "max" => "high",
            other => other,
        };
        crate::json::set(&mut body, "reasoning", json!({"effort": effort}));
    }
    body
}

fn push_message(out: &mut Vec<Value>, m: &Message) {
    let text: Vec<&str> = m
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    match m.role {
        Role::Assistant => {
            let calls: Vec<Value> = m
                .content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolUse { id, name, input } => Some(json!({
                        "id": id, "type": "function",
                        "function": {"name": name, "arguments": input.to_string()},
                    })),
                    _ => None,
                })
                .collect();
            let mut msg = json!({"role": "assistant", "content": text.join("\n")});
            if !calls.is_empty() {
                crate::json::set(&mut msg, "tool_calls", Value::Array(calls));
            }
            out.push(msg);
        }
        Role::User => {
            for b in &m.content {
                if let Block::ToolResult { tool_use_id, content, is_error } = b {
                    let content = if *is_error { format!("Error: {content}") } else { content.clone() };
                    out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}));
                }
            }
            if !text.is_empty() {
                out.push(json!({"role": "user", "content": text.join("\n\n")}));
            }
        }
    }
}

/// Read a chat completions response. `input_tokens` excludes cached prompt
/// tokens, as on the Messages API.
pub fn parse(v: &Value) -> Result<ModelResponse, ModelError> {
    let choice = &crate::json::at(v, "/choices/0");
    if choice.is_null() {
        return Err(ModelError::Retryable(format!("no choices in {v}")));
    }
    let msg = &crate::json::at(choice, "/message");
    let mut content = Vec::new();
    if let Some(text) = crate::json::at(msg, "/content").as_str().filter(|t| !t.trim().is_empty()) {
        content.push(Block::text(text));
    }
    for call in crate::json::at(msg, "/tool_calls").as_array().into_iter().flatten() {
        let raw = crate::json::at(call, "/function/arguments").as_str().unwrap_or("{}");
        let input = serde_json::from_str(raw).unwrap_or_else(|_| json!({"_unparsed_arguments": raw}));
        content.push(Block::ToolUse {
            id: crate::json::at(call, "/id").as_str().unwrap_or_default().to_owned(),
            name: crate::json::at(call, "/function/name").as_str().unwrap_or_default().to_owned(),
            input,
        });
    }
    let has_calls = content.iter().any(|b| matches!(b, Block::ToolUse { .. }));
    let stop = match crate::json::at(choice, "/finish_reason").as_str() {
        _ if has_calls => StopKind::ToolUse,
        Some("length") => StopKind::MaxTokens,
        _ => StopKind::EndTurn,
    };
    let u = &crate::json::at(v, "/usage");
    let cached = crate::json::at(u, "/prompt_tokens_details/cached_tokens").as_u64().unwrap_or(0);
    let usage = ModelUsage {
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: cached,
        input_tokens: crate::json::at(u, "/prompt_tokens").as_u64().unwrap_or(0).saturating_sub(cached),
        output_tokens: crate::json::at(u, "/completion_tokens").as_u64().unwrap_or(0),
    };
    let thought = crate::json::at(u, "/completion_tokens_details/reasoning_tokens").as_u64().unwrap_or(0) > 0
        || crate::json::at(msg, "/reasoning").as_str().is_some_and(|r| !r.is_empty());
    let cost = crate::json::at(u, "/cost").as_f64();
    Ok(ModelResponse { content, stop, usage, thought, cost })
}

#[async_trait]
impl ModelClient for OpenAiCompatModel {
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.base_url.trim_end_matches('/')))
            .bearer_auth(&self.api_key)
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
        if let Some(err) = v.get("error") {
            return Err(ModelError::Retryable(err.to_string()));
        }
        parse(&v)
    }
}

#[cfg(test)]
mod tests;
