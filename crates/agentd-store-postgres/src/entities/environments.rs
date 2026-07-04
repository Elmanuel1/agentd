//! `EnvironmentsStore` on Postgres — org-scoped CRUD.

use agentd_core::OrgId;
use agentd_environments::store::{EnvironmentsStore, StoreError};
use agentd_environments::types::{Environment, EnvironmentId};
use async_trait::async_trait;
use sqlx::types::Json;

use crate::{enum_str, sqlx_msg, PgBackend};

/// Postgres-backed environments store.
#[derive(Debug, Clone)]
pub struct PgEnvironmentsStore {
    backend: PgBackend,
}

impl PgEnvironmentsStore {
    /// Wrap a pool.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

#[async_trait]
impl EnvironmentsStore for PgEnvironmentsStore {
    async fn insert(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        sqlx::query(
            "insert into environments (org_id, environment_id, name, state, created_at, doc)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&org.0)
        .bind(env.id.as_str())
        .bind(&env.name)
        .bind(enum_str(&env.state))
        .bind(env.created_at.as_datetime())
        .bind(Json(&env))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError> {
        let row: Option<(Json<Environment>,)> = sqlx::query_as(
            "select doc from environments where org_id = $1 and environment_id = $2",
        )
        .bind(&org.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn replace(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update environments set name = $3, state = $4, doc = $5
             where org_id = $1 and environment_id = $2",
        )
        .bind(&org.0)
        .bind(env.id.as_str())
        .bind(&env.name)
        .bind(enum_str(&env.state))
        .bind(Json(&env))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn delete(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError> {
        let row: Option<(Json<Environment>,)> = sqlx::query_as(
            "delete from environments where org_id = $1 and environment_id = $2 returning doc",
        )
        .bind(&org.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn list(&self, org: &OrgId) -> Result<Vec<Environment>, StoreError> {
        let rows: Vec<(Json<Environment>,)> = sqlx::query_as(
            "select doc from environments where org_id = $1 order by created_at",
        )
        .bind(&org.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }
}
