//! `initialize` alone, for `mcp_oauth_validate`: the vaults' probe port,
//! answered by the loop's MCP client, so agentd has one.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use shared::net::Guard;
use vaults::refresh::HttpResponse;
use vaults::validate::{McpProbe, Probed};

use super::server::initialize_params;

/// Probes MCP servers through `guard`.
#[derive(Debug, Clone)]
pub struct Probe {
    guard: Guard,
    timeout: Duration,
}

impl Probe {
    /// A probe that waits `timeout` for an answer.
    pub fn new(guard: Guard, timeout: Duration) -> Self {
        Self { guard, timeout }
    }
}

#[async_trait]
impl McpProbe for Probe {
    async fn initialize(&self, url: &str, bearer: &str) -> Probed {
        let Ok(url) = url.parse::<reqwest::Url>() else { return Probed::Failed(None) };
        let Ok(client) = self.guard.client(&url, self.timeout).await else { return Probed::Failed(None) };
        let body =
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize_params("mcp")});
        let sent = client
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json, text/event-stream")
            .bearer_auth(bearer)
            .json(&body)
            .send()
            .await;
        let Ok(reply) = sent else { return Probed::Failed(None) };
        let status = reply.status();
        if status.is_success() {
            return Probed::Answered;
        }
        let quoted = HttpResponse::read(reply).await;
        match status {
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => Probed::Refused(quoted),
            _ => Probed::Failed(Some(quoted)),
        }
    }
}
