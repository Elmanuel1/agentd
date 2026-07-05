//! `EnvironmentsStore` on Postgres via Diesel — org-scoped CRUD.

use agentd_core::OrgId;
use agentd_environments::store::{EnvironmentsStore, StoreError};
use agentd_environments::types::{Environment, EnvironmentId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;

use crate::schema::environments;
use crate::{diesel_be, enum_str, pool_be, PgBackend};

/// Postgres-backed environments store (Diesel).
#[derive(Debug, Clone)]
pub struct PgEnvironmentsStore {
    backend: PgBackend,
}

impl PgEnvironmentsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn parse(doc: serde_json::Value) -> Result<Environment, StoreError> {
    serde_json::from_value(doc).map_err(|e| StoreError::Backend(e.to_string()))
}

#[derive(diesel::Insertable)]
#[diesel(table_name = environments)]
struct NewEnvironment {
    org_id: String,
    environment_id: String,
    name: String,
    state: String,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[async_trait]
impl EnvironmentsStore for PgEnvironmentsStore {
    async fn insert(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(environments::table)
            .values(NewEnvironment {
                org_id: org.0.clone(),
                environment_id: env.id.as_str().to_owned(),
                name: env.name.clone(),
                state: enum_str(&env.state),
                created_at: env.created_at.as_datetime(),
                doc: serde_json::to_value(&env).map_err(|e| StoreError::Backend(e.to_string()))?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = environments::table
            .filter(environments::org_id.eq(&org.0))
            .filter(environments::environment_id.eq(id.as_str()))
            .select(environments::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc = serde_json::to_value(&env).map_err(|e| StoreError::Backend(e.to_string()))?;
        let affected = diesel::update(
            environments::table
                .filter(environments::org_id.eq(&org.0))
                .filter(environments::environment_id.eq(env.id.as_str())),
        )
        .set((
            environments::name.eq(&env.name),
            environments::state.eq(enum_str(&env.state)),
            environments::doc.eq(doc),
        ))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError> {
        let existing = self.get(org, id).await?;
        diesel::delete(
            environments::table
                .filter(environments::org_id.eq(&org.0))
                .filter(environments::environment_id.eq(id.as_str())),
        )
        .execute(&mut self.backend.diesel.get().await.map_err(pool_be)?)
        .await
        .map_err(diesel_be)?;
        Ok(existing)
    }

    async fn list(&self, org: &OrgId) -> Result<Vec<Environment>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = environments::table
            .filter(environments::org_id.eq(&org.0))
            .order(environments::created_at.asc())
            .select(environments::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse).collect()
    }
}
