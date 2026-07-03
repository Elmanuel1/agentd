//! Agent persistence — trait + in-memory implementation.
//!
//! All reads/writes are workspace-scoped: an agent created under one
//! workspace is invisible to another (live-verified tenancy model). The
//! store keeps the full version history (agents are append-only per
//! version; `update` never rewrites an old version).

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::types::{Agent, AgentId};

/// Store-level errors (mapped to wire errors by the service).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such agent in this workspace.
    #[error("agent not found")]
    NotFound,
}

/// Persistence port for agents.
#[async_trait]
pub trait AgentsStore: Send + Sync {
    /// Insert version 1 of a new agent.
    async fn insert(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError>;
    /// Fetch the latest version.
    async fn latest(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Agent, StoreError>;
    /// Fetch a specific version.
    async fn version(
        &self,
        ws: &WorkspaceId,
        id: &AgentId,
        version: u64,
    ) -> Result<Agent, StoreError>;
    /// Append a new version (the service computes it as latest+1).
    async fn append_version(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError>;
    /// Replace the latest version in place (archive stamping only).
    async fn replace_latest(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError>;
    /// All versions of one agent, ascending.
    async fn versions(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Vec<Agent>, StoreError>;
    /// Latest version of every agent in the workspace, insertion order.
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Agent>, StoreError>;
}

/// In-memory store — the default for tests and single-node dev.
#[derive(Debug, Default)]
pub struct InMemoryAgentsStore {
    /// (workspace → agent id → versions ascending)
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<AgentId, Vec<Agent>>>>,
}

impl InMemoryAgentsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AgentsStore for InMemoryAgentsStore {
    async fn insert(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        g.entry(ws.clone())
            .or_default()
            .insert(agent.id.clone(), vec![agent]);
        Ok(())
    }

    async fn latest(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Agent, StoreError> {
        let g = self.inner.read();
        g.get(ws)
            .and_then(|m| m.get(id))
            .and_then(|v| v.last())
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn version(
        &self,
        ws: &WorkspaceId,
        id: &AgentId,
        version: u64,
    ) -> Result<Agent, StoreError> {
        let g = self.inner.read();
        g.get(ws)
            .and_then(|m| m.get(id))
            .and_then(|v| v.iter().find(|a| a.version == version))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn append_version(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let versions = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&agent.id))
            .ok_or(StoreError::NotFound)?;
        versions.push(agent);
        Ok(())
    }

    async fn replace_latest(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let versions = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&agent.id))
            .ok_or(StoreError::NotFound)?;
        match versions.last_mut() {
            Some(last) => {
                *last = agent;
                Ok(())
            }
            None => Err(StoreError::NotFound),
        }
    }

    async fn versions(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Vec<Agent>, StoreError> {
        let g = self.inner.read();
        g.get(ws)
            .and_then(|m| m.get(id))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Agent>, StoreError> {
        let g = self.inner.read();
        Ok(g.get(ws)
            .map(|m| m.values().filter_map(|v| v.last().cloned()).collect())
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AgentTag, ModelSpec, ModelSpeed};
    use agentd_core::{Metadata, Timestamp};

    fn ws(n: &str) -> WorkspaceId {
        WorkspaceId(n.to_owned())
    }

    fn agent(version: u64) -> Agent {
        Agent {
            archived_at: None,
            created_at: Timestamp::now(),
            description: None,
            id: AgentId::generate(),
            mcp_servers: vec![],
            metadata: Metadata::new(),
            model: ModelSpec {
                id: "claude-sonnet-4-6".into(),
                speed: ModelSpeed::Standard,
            },
            multiagent: None,
            name: "t".into(),
            skills: vec![],
            system: None,
            tools: vec![],
            tag: AgentTag::Agent,
            updated_at: Timestamp::now(),
            version,
        }
    }

    #[tokio::test]
    async fn insert_and_fetch_latest() {
        let s = InMemoryAgentsStore::new();
        let a = agent(1);
        let id = a.id.clone();
        s.insert(&ws("w1"), a).await.unwrap();
        assert_eq!(s.latest(&ws("w1"), &id).await.unwrap().version, 1);
    }

    #[tokio::test]
    async fn workspace_isolation() {
        let s = InMemoryAgentsStore::new();
        let a = agent(1);
        let id = a.id.clone();
        s.insert(&ws("w1"), a).await.unwrap();
        // Same id, different workspace → not found.
        assert_eq!(
            s.latest(&ws("w2"), &id).await.unwrap_err(),
            StoreError::NotFound
        );
        assert!(s.list(&ws("w2")).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn version_history_appends() {
        let s = InMemoryAgentsStore::new();
        let mut a = agent(1);
        let id = a.id.clone();
        s.insert(&ws("w1"), a.clone()).await.unwrap();
        a.version = 2;
        s.append_version(&ws("w1"), a).await.unwrap();
        let versions = s.versions(&ws("w1"), &id).await.unwrap();
        assert_eq!(
            versions.iter().map(|a| a.version).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(s.latest(&ws("w1"), &id).await.unwrap().version, 2);
        assert_eq!(s.version(&ws("w1"), &id, 1).await.unwrap().version, 1);
    }

    #[tokio::test]
    async fn list_returns_latest_of_each() {
        let s = InMemoryAgentsStore::new();
        let mut a = agent(1);
        s.insert(&ws("w1"), a.clone()).await.unwrap();
        a.version = 2;
        s.append_version(&ws("w1"), a).await.unwrap();
        let b = agent(1);
        s.insert(&ws("w1"), b).await.unwrap();
        let listed = s.list(&ws("w1")).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().any(|x| x.version == 2));
    }

    #[tokio::test]
    async fn append_to_missing_agent_fails() {
        let s = InMemoryAgentsStore::new();
        assert_eq!(
            s.append_version(&ws("w1"), agent(2)).await.unwrap_err(),
            StoreError::NotFound
        );
    }
}
