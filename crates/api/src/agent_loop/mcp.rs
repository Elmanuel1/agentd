//! What the loop reaches MCP servers with: the stored connections, the
//! vaults' credentials, and each session's environment, read in the
//! organization its work item names. Every session has a work item from its
//! create, while a workspace key may name a workspace with no row.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use engine::mcp::{Mcp, McpEnvironments, Probe};
use environments::store::EnvironmentsStore;
use environments::types::EnvironmentConfig;
use parking_lot::RwLock;
use shared::net::Guard;
use shared::{BackendError, OrgId, WorkspaceId};
use vaults::{McpReach, VaultsService};
use work::store::Reach;
use work::WorkReads;

use crate::backends::Stores;

/// How long one of the loop's MCP requests may take: live answers a call its
/// server never answers after about 125 s (MC-26).
const MCP_TIMEOUT: Duration = Duration::from_secs(125);
/// How long `mcp_oauth_validate`'s `initialize` probe may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Environments read by workspace: a workspace's organization never changes,
/// so each is looked up once per process.
struct Environments {
    work: Arc<dyn WorkReads>,
    environments: Arc<dyn EnvironmentsStore>,
    orgs: RwLock<HashMap<String, OrgId>>,
}

impl Environments {
    async fn org(&self, ws: &WorkspaceId, session: &str) -> Result<OrgId, BackendError> {
        if let Some(org) = self.orgs.read().get(ws.as_str()) {
            return Ok(org.clone());
        }
        let item = self.work.get(ws, session, &Reach::default()).await.map_err(BackendError::new)?;
        let org =
            item.map(|i| i.org).ok_or_else(|| BackendError::new(std::io::Error::other("no work item")))?;
        self.orgs.write().insert(ws.as_str().to_owned(), org.clone());
        Ok(org)
    }
}

#[async_trait]
impl McpEnvironments for Environments {
    async fn config(
        &self,
        ws: &WorkspaceId,
        session: &str,
        environment_id: &str,
    ) -> Result<EnvironmentConfig, BackendError> {
        let org = self.org(ws, session).await?;
        let id = environment_id.parse().map_err(BackendError::new)?;
        Ok(self.environments.get(&org, &id).await.map_err(BackendError::new)?.config)
    }

    async fn org(&self, ws: &WorkspaceId, session: &str) -> Result<OrgId, BackendError> {
        Environments::org(self, ws, session).await
    }
}

/// The loop's MCP reach over `stores`, connecting where `guard` allows and
/// sending a failed refresh to `webhooks`' endpoints.
pub fn mcp(stores: &Stores, guard: Guard, webhooks: &Arc<webhooks::Webhooks>) -> Mcp {
    Mcp {
        connections: stores.mcp_connections.clone(),
        vaults: Arc::new(
            VaultsService::new(stores.vaults.clone(), stores.vault_credentials.clone())
                .with_mcp(McpReach {
                    probe: Arc::new(Probe::new(guard.clone(), PROBE_TIMEOUT)),
                    guard: guard.clone(),
                })
                .with_webhooks(webhooks.clone()),
        ),
        environments: Arc::new(Environments {
            work: stores.work_reads.clone(),
            environments: stores.environments.clone(),
            orgs: RwLock::default(),
        }),
        guard,
        timeout: MCP_TIMEOUT,
    }
}
