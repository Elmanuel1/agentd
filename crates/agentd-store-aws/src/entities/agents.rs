//! `AgentsStore` on DynamoDB — versioned entity pattern.
//!
//! Layout: table `{prefix}_agents`, `pk = workspace`, `sk = {agent_id}#{version:020}`.
//! Every version is its own item (append-only); `latest` = the item with the
//! highest `sk` under the agent's prefix.

use agentd_agents::store::{AgentsStore, StoreError};
use agentd_agents::{Agent, AgentSnapshot};
use agentd_agents::types::AgentId;
use agentd_core::WorkspaceId;
use async_trait::async_trait;

use crate::entities::vseg;
use crate::{AwsBackend, AwsError};

/// DynamoDB-backed agents store.
#[derive(Debug, Clone)]
pub struct DynamoAgentsStore {
    aws: AwsBackend,
}

impl DynamoAgentsStore {
    /// Build; ensures the table exists.
    pub async fn new(aws: AwsBackend) -> Result<Self, AwsError> {
        aws.dynamo.ensure_table(&aws.table("agents")).await?;
        Ok(Self { aws })
    }

    fn table(&self) -> String {
        self.aws.table("agents")
    }

    fn sk(id: &AgentId, version: u64) -> String {
        format!("{}#{}", id.as_str(), vseg(version))
    }

    fn prefix(id: &AgentId) -> String {
        format!("{}#", id.as_str())
    }
}

fn be(e: AwsError) -> StoreError {
    StoreError::Backend(e.to_string())
}

#[async_trait]
impl AgentsStore for DynamoAgentsStore {
    async fn insert(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        self.aws
            .dynamo
            .put(&self.table(), &ws.0, &Self::sk(&agent.id, agent.version), &agent)
            .await
            .map_err(be)
    }

    async fn latest(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Agent, StoreError> {
        let mut versions: Vec<Agent> = self
            .aws
            .dynamo
            .query(&self.table(), &ws.0, Some(&Self::prefix(id)))
            .await
            .map_err(be)?;
        versions.sort_by_key(|a| a.version);
        versions.pop().ok_or(StoreError::NotFound)
    }

    async fn version(
        &self,
        ws: &WorkspaceId,
        id: &AgentId,
        version: u64,
    ) -> Result<Agent, StoreError> {
        self.aws
            .dynamo
            .get(&self.table(), &ws.0, &Self::sk(id, version))
            .await
            .map_err(be)?
            .ok_or(StoreError::NotFound)
    }

    async fn append_version(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        // Append is only valid for an existing agent.
        let exists: Vec<Agent> = self
            .aws
            .dynamo
            .query(&self.table(), &ws.0, Some(&Self::prefix(&agent.id)))
            .await
            .map_err(be)?;
        if exists.is_empty() {
            return Err(StoreError::NotFound);
        }
        self.insert(ws, agent).await
    }

    async fn replace_latest(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        // Overwrite the item at the current highest version in place.
        let current = self.latest(ws, &agent.id).await?;
        self.aws
            .dynamo
            .put(&self.table(), &ws.0, &Self::sk(&agent.id, current.version), &agent)
            .await
            .map_err(be)
    }

    async fn versions(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Vec<Agent>, StoreError> {
        let mut versions: Vec<Agent> = self
            .aws
            .dynamo
            .query(&self.table(), &ws.0, Some(&Self::prefix(id)))
            .await
            .map_err(be)?;
        if versions.is_empty() {
            return Err(StoreError::NotFound);
        }
        versions.sort_by_key(|a| a.version);
        Ok(versions)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Agent>, StoreError> {
        let all: Vec<Agent> = self
            .aws
            .dynamo
            .query(&self.table(), &ws.0, None)
            .await
            .map_err(be)?;
        // Latest of each agent id, insertion-stable by id.
        let mut latest: std::collections::BTreeMap<String, Agent> = std::collections::BTreeMap::new();
        for a in all {
            let key = a.id.as_str().to_owned();
            match latest.get(&key) {
                Some(existing) if existing.version >= a.version => {}
                _ => {
                    latest.insert(key, a);
                }
            }
        }
        Ok(latest.into_values().collect())
    }
}

// Keeps AgentSnapshot referenced for downstream crates that re-export via
// this store module's public surface.
#[allow(dead_code)]
fn _snapshot_marker(a: &Agent) -> AgentSnapshot {
    a.snapshot()
}
