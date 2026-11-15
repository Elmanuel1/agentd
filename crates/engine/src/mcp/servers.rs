//! One step's view of a thread agent's MCP servers: what is stored of each,
//! the credential it connects with, and whether it already failed this turn.

use agents::types::McpServer;
use agents::AgentSnapshot;
use serde_json::Value;
use sessions::events::{Event, EventPayload, SessionEvent};
use sessions::mcp::McpConnection;
use sessions::types::{Session, SessionId};
use shared::{BackendError, ContentBlock, WorkspaceId};
use vaults::Credential;

use super::client::{Failure, Rpc};
use super::errors::{is_mcp_error, session_error, unavailable};
use super::server::{self, Called};
use super::Mcp;

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
}

impl std::fmt::Debug for Servers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Servers").field("session", &self.session).finish_non_exhaustive()
    }
}

/// The servers that failed since the turn started: the log after its last
/// `session.status_running`.
fn failed_this_turn(log: &[Event]) -> Vec<&str> {
    let turn = log
        .iter()
        .rposition(|e| matches!(e.payload, EventPayload::Session(SessionEvent::StatusRunning {})))
        .map_or(log, |i| log.get(i..).unwrap_or_default());
    turn.iter().filter_map(is_mcp_error).collect()
}

impl Mcp {
    /// `agent`'s servers in `session`, read in three statements (its
    /// environment, its stored connections, its credentials); none when the
    /// agent declares no server.
    pub async fn servers(
        &self,
        ws: &WorkspaceId,
        session: &Session,
        agent: &AgentSnapshot,
        log: &[Event],
    ) -> Result<Servers, BackendError> {
        let mut servers =
            Servers { mcp: None, ws: ws.clone(), session: session.id.clone(), list: Vec::new() };
        if agent.config.mcp_servers.is_empty() {
            return Ok(servers);
        }
        let env = self.environments.config(ws, session.id.as_str(), &session.environment_id).await?;
        let stored = self.connections.connections(ws, &session.id).await.map_err(BackendError::new)?;
        let urls: Vec<String> =
            agent.config.mcp_servers.iter().map(|McpServer::Url { url, .. }| url.clone()).collect();
        let mut creds = self.vaults.mcp_credentials(ws, &session.vault_ids, &urls).await?;
        let failed = failed_this_turn(log);
        servers.list = agent
            .config
            .mcp_servers
            .iter()
            .map(|McpServer::Url { name, url }| Server {
                name: name.clone(),
                url: url.clone(),
                allowed: reqwest::Url::parse(url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| env.mcp_allowed(h)))
                    .unwrap_or(false),
                credential: creds.remove(url),
                stored: stored.iter().find(|s| s.server_url == *url).cloned(),
                failed: failed.contains(&name.as_str()).then_some(Failure::Unreachable),
                rpc: None,
            })
            .collect();
        servers.mcp = Some(self.clone());
        Ok(servers)
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
        }
        Ok(errors)
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

    async fn token(&self, i: usize) -> Result<Option<String>, BackendError> {
        let (Some(mcp), Some(cred)) = (&self.mcp, self.list.get(i).and_then(|s| s.credential.as_ref()))
        else {
            return Ok(None);
        };
        Ok(mcp.vaults.mcp_token(&self.ws, cred).await?.map(|t| t.bearer.expose().to_owned()))
    }

    /// Open the stored session again, reading the credential's token.
    async fn resume(&mut self, i: usize) -> Result<Result<(), Failure>, BackendError> {
        let bearer = self.token(i).await?;
        let (Some(mcp), Some(s)) = (self.mcp.clone(), self.list.get_mut(i)) else {
            return Ok(Err(Failure::Unreachable));
        };
        let session = s.stored.as_ref().and_then(|c| c.mcp_session_id.clone());
        let opened = match s.allowed {
            true => Rpc::open(&mcp.guard, &s.url, bearer, session, mcp.timeout).await,
            false => Err(Failure::Unreachable),
        };
        match opened {
            Ok(rpc) => {
                s.rpc = Some(rpc);
                Ok(Ok(()))
            }
            Err(f) => {
                s.failed = Some(f.clone());
                Ok(Err(f))
            }
        }
    }

    /// Connect server `i` afresh and store what it lists; the
    /// `session.error` body when it fails.
    async fn reach(&mut self, i: usize) -> Result<Result<(), Value>, BackendError> {
        let Some(mcp) = self.mcp.clone() else { return Ok(Ok(())) };
        let allowed = self.list.get(i).is_some_and(|s| s.allowed);
        let bearer = match allowed {
            true => self.token(i).await?,
            false => None,
        };
        let Some(s) = self.list.get_mut(i) else { return Ok(Ok(())) };
        let opened = match allowed {
            true => connect(&mcp, &s.url, bearer).await,
            false => Err(Failure::Unreachable),
        };
        match opened {
            Ok((rpc, tools)) => {
                let row = McpConnection {
                    server_url: s.url.clone(),
                    mcp_session_id: rpc.session.clone(),
                    tools: Value::Array(tools),
                    credential_updated_at: s.credential.as_ref().map(|c| c.updated_at),
                };
                mcp.connections.save(&self.ws, &self.session, &row).await.map_err(BackendError::new)?;
                s.stored = Some(row);
                s.rpc = Some(rpc);
                s.failed = None;
                Ok(Ok(()))
            }
            Err(f) => {
                let error = session_error(&s.name, "initialize", &f);
                s.failed = Some(f);
                s.rpc = None;
                Ok(Err(error))
            }
        }
    }

    /// List server `i`'s tools again and store them (MC-16).
    async fn relist(&mut self, i: usize) -> Result<(), BackendError> {
        let Some(mcp) = self.mcp.clone() else { return Ok(()) };
        let Some(s) = self.list.get_mut(i) else { return Ok(()) };
        let (Some(rpc), Some(stored)) = (s.rpc.as_mut(), s.stored.as_mut()) else { return Ok(()) };
        if let Ok(tools) = server::list(rpc).await {
            stored.tools = Value::Array(tools);
            stored.mcp_session_id = rpc.session.clone();
            mcp.connections.save(&self.ws, &self.session, stored).await.map_err(BackendError::new)?;
        }
        Ok(())
    }
}

async fn connect(mcp: &Mcp, url: &str, bearer: Option<String>) -> Result<(Rpc, Vec<Value>), Failure> {
    let mut rpc = Rpc::open(&mcp.guard, url, bearer, None, mcp.timeout).await?;
    server::initialize(&mut rpc).await?;
    let tools = server::list(&mut rpc).await?;
    Ok((rpc, tools))
}

fn failed(server: &str, stage: &str, f: &Failure) -> CallOutcome {
    CallOutcome::Failed { content: unavailable(server), error: session_error(server, stage, f) }
}
