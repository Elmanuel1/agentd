//! `AgentsStore` on Postgres via Diesel — versioned rows, latest via
//! `DISTINCT ON`. The typed schema lives in `crate::schema`; the wire object is
//! kept in the `doc` jsonb column (fetched as `serde_json::Value`).

use agentd_agents::store::{AgentsStore, StoreError};
use agentd_agents::types::AgentId;
use agentd_agents::Agent;
use agentd_core::{Pagination, WorkspaceId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;

use crate::schema::agents;
use crate::{diesel_be, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres-backed agents store (Diesel).
#[derive(Debug, Clone)]
pub struct PgAgentsStore {
    backend: PgBackend,
}

impl PgAgentsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

/// Row for inserts — the lifted filter columns plus the wire `doc`.
#[derive(diesel::Insertable)]
#[diesel(table_name = agents)]
struct NewAgent {
    workspace_id: String,
    agent_id: String,
    version: i64,
    name: String,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

fn row(ws: &WorkspaceId, a: &Agent) -> Result<NewAgent, StoreError> {
    Ok(NewAgent {
        workspace_id: ws.as_str().to_owned(),
        agent_id: a.id.as_str().to_owned(),
        version: a.version as i64,
        name: a.name.clone(),
        archived_at: a.archived_at.map(|t| t.as_datetime()),
        created_at: a.created_at.as_datetime(),
        doc: to_doc(a)?,
    })
}

#[async_trait]
impl AgentsStore for PgAgentsStore {
    async fn insert(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(agents::table)
            .values(row(ws, &agent)?)
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn latest(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Agent, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = agents::table
            .filter(agents::workspace_id.eq(ws.as_str()))
            .filter(agents::agent_id.eq(id.as_str()))
            .order(agents::version.desc())
            .select(agents::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn version(&self, ws: &WorkspaceId, id: &AgentId, version: u64) -> Result<Agent, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = agents::table
            .filter(agents::workspace_id.eq(ws.as_str()))
            .filter(agents::agent_id.eq(id.as_str()))
            .filter(agents::version.eq(version as i64))
            .select(agents::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn append_version(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let exists: Option<i64> = agents::table
            .filter(agents::workspace_id.eq(ws.as_str()))
            .filter(agents::agent_id.eq(agent.id.as_str()))
            .select(agents::version)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        if exists.is_none() {
            return Err(StoreError::NotFound);
        }
        diesel::insert_into(agents::table)
            .values(row(ws, &agent)?)
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn replace_latest(&self, ws: &WorkspaceId, agent: Agent) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let max: Option<i64> = agents::table
            .filter(agents::workspace_id.eq(ws.as_str()))
            .filter(agents::agent_id.eq(agent.id.as_str()))
            .select(diesel::dsl::max(agents::version))
            .first(&mut conn)
            .await
            .map_err(diesel_be)?;
        let Some(maxv) = max else {
            return Err(StoreError::NotFound);
        };
        let doc = to_doc(&agent)?;
        let affected = diesel::update(
            agents::table
                .filter(agents::workspace_id.eq(ws.as_str()))
                .filter(agents::agent_id.eq(agent.id.as_str()))
                .filter(agents::version.eq(maxv)),
        )
        .set((
            agents::doc.eq(doc),
            agents::name.eq(&agent.name),
            agents::archived_at.eq(agent.archived_at.map(|t| t.as_datetime())),
        ))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn versions(&self, ws: &WorkspaceId, id: &AgentId) -> Result<Vec<Agent>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = agents::table
            .filter(agents::workspace_id.eq(ws.as_str()))
            .filter(agents::agent_id.eq(id.as_str()))
            .order(agents::version.asc())
            .select(agents::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        if docs.is_empty() {
            return Err(StoreError::NotFound);
        }
        docs.into_iter().map(parse_doc).collect()
    }

    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Agent>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // "Latest of each agent" — Postgres DISTINCT ON, one indexed query.
        let docs: Vec<serde_json::Value> = agents::table
            .filter(agents::workspace_id.eq(ws.as_str()))
            .distinct_on(agents::agent_id)
            .order((agents::agent_id, agents::version.desc()))
            .select(agents::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }
}
