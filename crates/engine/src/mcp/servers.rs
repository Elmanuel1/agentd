//! One step's view of a thread agent's MCP servers: what is stored of each,
//! the credential it connects with, and whether it already failed this turn.

use serde_json::Value;
use sessions::mcp::McpConnection;
use sessions::types::SessionId;
use shared::{BackendError, ContentBlock, WorkspaceId};
use vaults::Credential;

use super::client::{Failure, Rpc};
use super::errors::{session_error, unavailable};
use super::server::{self, Called};
use super::Mcp;

mod open;
mod reach;

/// A tool one of the agent's servers lists.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    /// The server's name in the agent's `mcp_servers`.
    pub server: String,
    /// The tool's name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// Its input schema.
    pub input_schema: Value,
}

/// What a call to an MCP tool came to.
#[derive(Debug, Clone, PartialEq)]
pub enum CallOutcome {
    /// The server answered.
    Called(Called),
    /// Another worker is refreshing the server's token: the step runs
    /// again a second later (MC-20, MC-23).
    Deferred,
    /// The server could not be reached: the call's error content, and the
    /// `session.error` to record for it (MC-26).
    Failed {
        /// The `is_error` result's content.
        content: Vec<ContentBlock>,
        /// The `session.error` body.
        error: Value,
    },
}

struct Server {
    name: String,
    url: String,
    allowed: bool,
    credential: Option<Credential>,
    stored: Option<McpConnection>,
    /// Why the server failed in this turn, when it did.
    failed: Option<Failure>,
    rpc: Option<Rpc>,
}

impl Server {
    /// The stored connection was made with the credential the step reads.
    fn current(&self) -> bool {
        self.stored
            .as_ref()
            .is_some_and(|s| s.credential_updated_at == self.credential.as_ref().map(|c| c.updated_at))
    }
}

/// The servers of one step.
pub struct Servers {
    mcp: Option<Mcp>,
    ws: WorkspaceId,
    session: SessionId,
    list: Vec<Server>,
    deferred: bool,
    org: Option<shared::OrgId>,
}

impl std::fmt::Debug for Servers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Servers").field("session", &self.session).finish_non_exhaustive()
    }
}

impl Servers {
    /// Connect each server that needs it, and the `session.error` of each
    /// that fails. At a turn's start every server connects again (MC-15),
    /// failed or not (MC-14); later in the turn only one with no current
    /// stored connection that has not failed this turn.
    pub async fn connect(&mut self, starting: bool) -> Result<Vec<Value>, BackendError> {
        let mut errors = Vec::new();
        for i in 0..self.list.len() {
            let Some(s) = self.list.get_mut(i) else { continue };
            if starting {
                s.failed = None;
            } else if s.failed.is_some() || s.current() {
                continue;
            }
            if let Err(e) = self.reach(i).await? {
                errors.push(e);
            }
            if self.deferred {
                break;
            }
        }
        Ok(errors)
    }

    /// A refresh another worker holds deferred the step (MC-20).
    pub fn deferred(&self) -> bool {
        self.deferred
    }

    /// The tools of every server whose connection holds, in the agent's
    /// server order.
    pub fn tools(&self) -> Vec<McpTool> {
        self.list
            .iter()
            .filter(|s| s.failed.is_none())
            .filter_map(|s| Some((s, s.stored.as_ref()?.tools.as_array()?)))
            .flat_map(|(s, tools)| {
                tools.iter().filter_map(|t| {
                    Some(McpTool {
                        server: s.name.clone(),
                        name: t.get("name")?.as_str()?.to_owned(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        input_schema: t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({"type": "object"})),
                    })
                })
            })
            .collect()
    }

    /// Call `tool` of `server` with `input`. A stored session the server
    /// ended (404) reconnects once and calls again.
    pub async fn call(
        &mut self,
        server: &str,
        tool: &str,
        input: &Value,
    ) -> Result<CallOutcome, BackendError> {
        let Some(i) = self.list.iter().position(|s| s.name == server) else {
            return Ok(failed(server, "tools/call", &Failure::Unreachable));
        };
        for attempt in 0..2 {
            if let Some(f) = self.list.get(i).and_then(|s| s.failed.clone()) {
                return Ok(failed(server, "tools/call", &f));
            }
            let (open, current) = self.list.get(i).map_or((false, false), |s| (s.rpc.is_some(), s.current()));
            if !open {
                let reached = match current {
                    true => self.resume(i).await?.map_err(|f| session_error(server, "initialize", &f)),
                    false => self.reach(i).await?,
                };
                if self.deferred {
                    return Ok(CallOutcome::Deferred);
                }
                if let Err(error) = reached {
                    return Ok(CallOutcome::Failed { content: unavailable(server), error });
                }
            }
            let Some(rpc) = self.list.get_mut(i).and_then(|s| s.rpc.as_mut()) else { break };
            let answered = server::call(rpc, tool, input).await;
            let Some(s) = self.list.get_mut(i) else { break };
            match answered {
                Ok(called) => {
                    if called.list_changed {
                        self.relist(i).await?;
                    }
                    return Ok(CallOutcome::Called(called));
                }
                Err(Failure::Gone) if attempt == 0 => {
                    s.rpc = None;
                    s.stored = None;
                }
                Err(f) => {
                    s.rpc = None;
                    s.failed = Some(f.clone());
                    return Ok(failed(server, "tools/call", &f));
                }
            }
        }
        Ok(failed(server, "tools/call", &Failure::Gone))
    }
}

fn failed(server: &str, stage: &str, f: &Failure) -> CallOutcome {
    CallOutcome::Failed { content: unavailable(server), error: session_error(server, stage, f) }
}
