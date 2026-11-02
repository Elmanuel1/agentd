//! Agent persistence — trait + in-memory implementation.
//!
//! All reads/writes are workspace-scoped: an agent created under one
//! workspace is invisible to another (live-verified tenancy model). The
//! store keeps the full version history (agents are append-only per
//! version; `update` never rewrites an old version).

use async_trait::async_trait;
use shared::{Pagination, WorkspaceId};

use crate::types::{Agent, AgentId};

/// Store-level errors (mapped to wire errors by the service).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such agent in this workspace.
    #[error("agent not found")]
    NotFound,
    /// The backing store failed (infrastructure error).
    #[error("store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// Persistence port for agents.
#[async_trait]
pub trait AgentsStore: Send + Sync {
    /// Insert version 1 of a new agent.
    async fn insert(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError>;
    /// Fetch the latest version.
    async fn latest(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Agent, StoreError>;
    /// Fetch a specific version.
    async fn version(&self, ws: &WorkspaceId, id: &AgentId, version: u64) -> Result<Agent, StoreError>;
    /// Append a new version (the service computes it as latest+1).
    async fn append_version(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError>;
    /// Replace the latest version in place (archive stamping only).
    async fn replace_latest(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError>;
    /// All versions of one agent, ascending. Version history is bounded and
    /// several internal callers need the full set, so it is not paginated
    /// DB-side (the route slices in memory).
    async fn versions(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Vec<Agent>, StoreError>;
    /// One page of the latest version of every agent in the workspace,
    /// insertion order. `page` is applied DB-side (fetch `limit + 1`).
    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Agent>, StoreError>;
}
