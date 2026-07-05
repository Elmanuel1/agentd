//! Agent persistence — trait + in-memory implementation.
//!
//! All reads/writes are workspace-scoped: an agent created under one
//! workspace is invisible to another (live-verified tenancy model). The
//! store keeps the full version history (agents are append-only per
//! version; `update` never rewrites an old version).

use agentd_core::WorkspaceId;
use async_trait::async_trait;

use crate::types::{Agent, AgentId};

/// Store-level errors (mapped to wire errors by the service).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such agent in this workspace.
    #[error("agent not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(String),
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
