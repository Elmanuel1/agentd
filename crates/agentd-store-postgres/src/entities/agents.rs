//! `AgentsStore` on Postgres — versioned rows, latest via `DISTINCT ON`.

use agentd_agents::store::{AgentsStore, StoreError};
use agentd_agents::types::AgentId;
use agentd_agents::Agent;
use agentd_core::WorkspaceId;
use async_trait::async_trait;
use sqlx::types::Json;

use crate::{sqlx_msg, PgBackend};

/// Postgres-backed agents store.
#[derive(Debug, Clone)]
pub struct PgAgentsStore {
    backend: PgBackend,
}

impl PgAgentsStore {
    /// Wrap a pool.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

#[async_trait]
impl AgentsStore for PgAgentsStore {
    async fn insert(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        sqlx::query(
            "insert into agents (workspace_id, agent_id, version, name, archived_at, created_at, doc)
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&ws.0)
        .bind(agent.id.as_str())
        .bind(agent.version as i64)
        .bind(&agent.name)
        .bind(agent.archived_at.map(|t| t.as_datetime()))
        .bind(agent.created_at.as_datetime())
        .bind(Json(&agent))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn latest(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Agent, StoreError> {
        let row: Option<(Json<Agent>,)> = sqlx::query_as(
            "select doc from agents where workspace_id = $1 and agent_id = $2
             order by version desc limit 1",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn version(
        &self,
        ws: &WorkspaceId,
        id: &AgentId,
        version: u64,
    ) -> Result<Agent, StoreError> {
        let row: Option<(Json<Agent>,)> = sqlx::query_as(
            "select doc from agents where workspace_id = $1 and agent_id = $2 and version = $3",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .bind(version as i64)
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn append_version(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let exists: Option<(i32,)> = sqlx::query_as(
            "select 1 from agents where workspace_id = $1 and agent_id = $2 limit 1",
        )
        .bind(&ws.0)
        .bind(agent.id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        if exists.is_none() {
            return Err(StoreError::NotFound);
        }
        self.insert(ws, agent).await
    }

    async fn replace_latest(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update agents set doc = $3, name = $4, archived_at = $5
             where workspace_id = $1 and agent_id = $2
               and version = (select max(version) from agents
                              where workspace_id = $1 and agent_id = $2)",
        )
        .bind(&ws.0)
        .bind(agent.id.as_str())
        .bind(Json(&agent))
        .bind(&agent.name)
        .bind(agent.archived_at.map(|t| t.as_datetime()))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn versions(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Vec<Agent>, StoreError> {
        let rows: Vec<(Json<Agent>,)> = sqlx::query_as(
            "select doc from agents where workspace_id = $1 and agent_id = $2 order by version asc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        if rows.is_empty() {
            return Err(StoreError::NotFound);
        }
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Agent>, StoreError> {
        // "Latest of each agent" — one indexed query, no app-side fold.
        let rows: Vec<(Json<Agent>,)> = sqlx::query_as(
            "select distinct on (agent_id) doc from agents
             where workspace_id = $1 order by agent_id, version desc",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }
}
