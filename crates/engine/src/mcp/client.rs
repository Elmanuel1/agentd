//! JSON-RPC to one MCP server over streamable HTTP: each request a POST whose
//! answer is either a JSON body or an event stream that carries it, with the
//! server's `Mcp-Session-Id` sent back on every request after `initialize`.

use std::time::Duration;

use reqwest::header::{HeaderMap, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::StatusCode;
use serde_json::{json, Value};
use shared::net::{Guard, NetError};

/// The protocol version agentd speaks, as live's client falls back to it.
pub const PROTOCOL: &str = "2025-11-25";
const SESSION: &str = "mcp-session-id";
const VERSION: &str = "mcp-protocol-version";

/// Why a request got no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The host does not resolve, refuses, times out or resolves inward.
    Unreachable,
    /// The server answered 401 to the credential the loop sent.
    Unauthorized,
    /// The server answered 401 to a static bearer token.
    Rejected,
    /// The server answered 401, and the session's vaults hold no credential
    /// for its URL.
    Unauthenticated,
    /// The server answered 403.
    Forbidden,
    /// The server answered 404 to a stored session id: it ended the session.
    Gone,
    /// Any other status, or an answer that is not JSON-RPC.
    Status(u16),
    /// The credential's token expired and its refresh failed.
    Refresh,
    /// Another worker is refreshing the credential's token.
    Busy,
}

impl From<NetError> for Failure {
    fn from(_: NetError) -> Self {
        Self::Unreachable
    }
}

/// A server's answer to one request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answer {
    /// `result`, when it succeeded.
    pub result: Option<Value>,
    /// The JSON-RPC `error`, when it failed.
    pub error: Option<Value>,
    /// The server sent `notifications/tools/list_changed` with it (MC-16).
    pub list_changed: bool,
}

/// One server, called with the step's credential.
pub struct Rpc {
    client: reqwest::Client,
    url: reqwest::Url,
    bearer: Option<String>,
    /// The server's session id, once `initialize` gave one.
    pub session: Option<String>,
    next: u64,
}

impl std::fmt::Debug for Rpc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rpc")
            .field("url", &self.url.as_str())
            .field("session", &self.session)
            .finish_non_exhaustive()
    }
}

impl Rpc {
    /// A caller of `url` through `guard`, which refuses an internal address
    /// before any request is sent (MC-11).
    pub async fn open(
        guard: &Guard,
        url: &str,
        bearer: Option<String>,
        session: Option<String>,
        timeout: Duration,
    ) -> Result<Self, Failure> {
        let url: reqwest::Url = url.parse().map_err(|_| Failure::Unreachable)?;
        let client = guard.client(&url, timeout).await?;
        Ok(Self { client, url, bearer, session, next: 1 })
    }

    fn headers(&self, bearer: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, reqwest::header::HeaderValue::from_static("application/json, text/event-stream"));
        h.insert(CONTENT_TYPE, reqwest::header::HeaderValue::from_static("application/json"));
        if let Some(Ok(v)) = self.bearer.as_ref().filter(|_| bearer).map(|b| format!("Bearer {b}").parse()) {
            h.insert(AUTHORIZATION, v);
        }
        if let Some(Ok(v)) = self.session.as_ref().map(|s| s.parse()) {
            h.insert(SESSION, v);
            h.insert(VERSION, reqwest::header::HeaderValue::from_static(PROTOCOL));
        }
        h
    }

    async fn send(&self, body: &Value, bearer: bool) -> Result<reqwest::Response, Failure> {
        let sent = self.client.post(self.url.clone()).headers(self.headers(bearer)).json(body).send().await;
        sent.map_err(|_| Failure::Unreachable)
    }

    async fn post(&mut self, body: &Value) -> Result<(HeaderMap, String), Failure> {
        let mut reply = self.send(body, true).await?;
        // Live sends a request its bearer was refused for once more with no
        // Authorization header (MC-4 and MC-26 capture).
        if reply.status() == StatusCode::UNAUTHORIZED && self.bearer.is_some() {
            reply = self.send(body, false).await?;
        }
        match reply.status() {
            s if s.is_success() => {}
            StatusCode::UNAUTHORIZED => return Err(Failure::Unauthorized),
            StatusCode::FORBIDDEN => return Err(Failure::Forbidden),
            StatusCode::NOT_FOUND if self.session.is_some() => return Err(Failure::Gone),
            s => return Err(Failure::Status(s.as_u16())),
        }
        let headers = reply.headers().clone();
        let text = reply.text().await.map_err(|_| Failure::Unreachable)?;
        Ok((headers, text))
    }

    /// Send `method` and wait for its answer.
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Answer, Failure> {
        let id = self.next;
        self.next += 1;
        let (headers, text) =
            self.post(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        if let Some(s) = headers.get(SESSION).and_then(|v| v.to_str().ok()) {
            self.session = Some(s.to_owned());
        }
        let streamed = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"));
        let messages = match streamed {
            true => events(&text),
            false => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Array(all)) => all,
                Ok(one) => vec![one],
                Err(_) => Vec::new(),
            },
        };
        let mut answer = Answer::default();
        let mut answered = false;
        for m in messages {
            if m.get("method").and_then(Value::as_str) == Some("notifications/tools/list_changed") {
                answer.list_changed = true;
            }
            if m.get("id").and_then(Value::as_u64) == Some(id) {
                answered = true;
                answer.result = m.get("result").cloned();
                answer.error = m.get("error").cloned();
            }
        }
        match answered {
            true => Ok(answer),
            false => Err(Failure::Status(200)),
        }
    }

    /// Send the notification `method`, which has no answer.
    pub async fn notify(&mut self, method: &str) -> Result<(), Failure> {
        self.post(&json!({"jsonrpc": "2.0", "method": method})).await.map(drop)
    }
}

/// The JSON messages of an event stream's `data` fields.
fn events(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .flat_map(|chunk| chunk.split("\r\n\r\n"))
        .filter_map(|event| {
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            (!data.is_empty()).then(|| serde_json::from_str(&data.join("\n")).ok()).flatten()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_stream_yields_each_data_message() {
        let text =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n\
                    id: 2\ndata: {\"jsonrpc\":\"2.0\",\n data: \"id\":1,\"result\":{}}\n\n";
        let got = events(text);
        assert_eq!(got.len(), 1, "a data line that does not start the field is not one");
        assert_eq!(got[0]["method"], "notifications/tools/list_changed");
        let split = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1,\"result\":{}}\n\n";
        assert_eq!(events(split)[0]["id"], 1);
    }
}
