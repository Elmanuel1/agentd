//! `EnvironmentsStore` on Postgres via Diesel — org-scoped CRUD.

use agentd_core::OrgId;
use agentd_environments::store::{EnvironmentsStore, StoreError};
use agentd_environments::types::{Environment, EnvironmentId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;

use crate::schema::environments;
use crate::{diesel_be, enum_str, parse_doc, pool_be, to_doc, PgBackend};

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
                doc: to_doc(&env)?,
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
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc = to_doc(&env)?;
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
        // Single round-trip: delete and return the row in one statement.
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = diesel::delete(
            environments::table
                .filter(environments::org_id.eq(&org.0))
                .filter(environments::environment_id.eq(id.as_str())),
        )
        .returning(environments::doc)
        .get_result(&mut conn)
        .await
        .optional()
        .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
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
        docs.into_iter().map(parse_doc).collect()
    }
}
