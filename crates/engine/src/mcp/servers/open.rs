//! A step's first read of its servers: the environment, the stored
//! connections and the credentials, in three statements.

use agents::AgentSnapshot;
use agents::types::McpServer;
use sessions::events::{Event, EventPayload, SessionEvent};
use sessions::types::Session;
use shared::{BackendError, WorkspaceId};

use super::super::Mcp;
use super::super::client::Failure;
use super::super::errors::is_mcp_error;
use super::{Server, Servers};

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
        let mut servers = Servers {
            mcp: None,
            ws: ws.clone(),
            session: session.id.clone(),
            list: Vec::new(),
            deferred: false,
            org: None,
        };
        if agent.config.mcp_servers.is_empty() {
            return Ok(servers);
        }
        let env = self.environments.config(ws, session.id.as_str(), &session.environment_id).await?;
        servers.org = Some(self.environments.org(ws, session.id.as_str()).await?);
        let stored = self.connections.connections(ws, &session.id).await.map_err(BackendError::new)?;
        let urls: Vec<String> =
            agent.config.mcp_servers.iter().map(|McpServer::Url { url, .. }| url.clone()).collect();
        let mut creds = self.vaults.mcp_credentials(ws, &session.vault_ids, &urls).await?;
        // The servers that failed since the turn started: the log after its
        // last `session.status_running`.
        let failed: Vec<&str> = {
            let turn = log
                .iter()
                .rposition(|e| matches!(e.payload, EventPayload::Session(SessionEvent::StatusRunning {})))
                .map_or(log, |i| log.get(i..).unwrap_or_default());
            turn.iter().filter_map(is_mcp_error).collect()
        };
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
