//! What the loop asks of one MCP server: connect (`initialize`, then
//! `notifications/initialized`, then `tools/list`), list its tools again, and
//! call one.

use serde_json::{json, Value};
use shared::ContentBlock;

use super::client::{Failure, Rpc, PROTOCOL};

/// The client agentd names at `initialize`, as live's does.
const CLIENT: (&str, &str) = ("mcp", "0.1.0");

/// What a tool call answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Called {
    /// Its content, as session events carry it.
    pub content: Vec<ContentBlock>,
    /// The tool reported an error.
    pub is_error: bool,
    /// The server's tool list changed with it.
    pub list_changed: bool,
}

/// Open the session: `rpc` comes back holding the server's session id.
pub async fn initialize(rpc: &mut Rpc) -> Result<(), Failure> {
    rpc.session = None;
    let params = json!({
        "protocolVersion": PROTOCOL,
        "capabilities": {},
        "clientInfo": {"name": CLIENT.0, "version": CLIENT.1},
    });
    let answer = rpc.request("initialize", params).await?;
    if answer.result.is_none() {
        return Err(Failure::Status(200));
    }
    rpc.notify("notifications/initialized").await
}

/// Every tool the server lists, across its pages.
pub async fn list(rpc: &mut Rpc) -> Result<Vec<Value>, Failure> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let params = match &cursor {
            Some(c) => json!({"cursor": c}),
            None => json!({}),
        };
        let result = rpc.request("tools/list", params).await?.result.ok_or(Failure::Status(200))?;
        tools.extend(result.get("tools").and_then(Value::as_array).cloned().unwrap_or_default());
        cursor = result.get("nextCursor").and_then(Value::as_str).map(str::to_owned);
        if cursor.is_none() {
            return Ok(tools);
        }
    }
}

/// Call `name` with `input`. A JSON-RPC error is the tool's error, not the
/// connection's.
pub async fn call(rpc: &mut Rpc, name: &str, input: &Value) -> Result<Called, Failure> {
    let answer = rpc.request("tools/call", json!({"name": name, "arguments": input})).await?;
    let list_changed = answer.list_changed;
    if let Some(error) = answer.error {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("The tool call failed.");
        return Ok(Called { content: vec![ContentBlock::text(message)], is_error: true, list_changed });
    }
    let result = answer.result.unwrap_or_default();
    let content = result.get("content").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(Called {
        content: content.iter().map(block).collect(),
        is_error: result.get("isError").and_then(Value::as_bool).unwrap_or(false),
        list_changed,
    })
}

/// An MCP content item as a session event's block: text and images as they
/// are, anything else as its JSON text.
fn block(item: &Value) -> ContentBlock {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => ContentBlock::text(item.get("text").and_then(Value::as_str).unwrap_or_default()),
        Some("image") => ContentBlock::Image {
            source: json!({"type": "base64", "media_type": item.get("mimeType"), "data": item.get("data")}),
        },
        _ => ContentBlock::text(item.to_string()),
    }
}
