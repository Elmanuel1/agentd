//! `DreamsStore` on Postgres — dream rows + append-only runs.

use agentd_core::WorkspaceId;
use agentd_dreams::store::{DreamsStore, StoreError};
use agentd_dreams::types::{Dream, DreamId, DreamRun};
use async_trait::async_trait;
use sqlx::types::Json;

use crate::{enum_str, sqlx_msg, PgBackend};

/// Postgres-backed dreams store.
#[derive(Debug, Clone)]
pub struct PgDreamsStore {
    backend: PgBackend,
}

impl PgDreamsStore {
    /// Wrap a pool.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

#[async_trait]
impl DreamsStore for PgDreamsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError> {
        sqlx::query(
            "insert into dreams (workspace_id, dream_id, name, status, created_at, doc)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&ws.0)
        .bind(d.id.as_str())
        .bind(&d.name)
        .bind(enum_str(&d.status))
        .bind(d.created_at.as_datetime())
        .bind(Json(&d))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Dream, StoreError> {
        let row: Option<(Json<Dream>,)> =
            sqlx::query_as("select doc from dreams where workspace_id = $1 and dream_id = $2")
                .bind(&ws.0)
                .bind(id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update dreams set name = $3, status = $4, doc = $5
             where workspace_id = $1 and dream_id = $2",
        )
        .bind(&ws.0)
        .bind(d.id.as_str())
        .bind(&d.name)
        .bind(enum_str(&d.status))
        .bind(Json(&d))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Dream>, StoreError> {
        let rows: Vec<(Json<Dream>,)> = sqlx::query_as(
            "select doc from dreams where workspace_id = $1 order by created_at desc",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DreamRun) -> Result<(), StoreError> {
        let exists: Option<(i32,)> =
            sqlx::query_as("select 1 from dreams where workspace_id = $1 and dream_id = $2")
                .bind(&ws.0)
                .bind(run.dream_id.as_str())
                .fetch_optional(&self.backend.pool)
                .await
                .map_err(be)?;
        if exists.is_none() {
            return Err(StoreError::NotFound);
        }
        sqlx::query(
            "insert into dream_runs (workspace_id, dream_id, run_id, created_at, doc)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(&ws.0)
        .bind(run.dream_id.as_str())
        .bind(run.id.as_str())
        .bind(run.created_at.as_datetime())
        .bind(Json(&run))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn runs(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Vec<DreamRun>, StoreError> {
        let rows: Vec<(Json<DreamRun>,)> = sqlx::query_as(
            "select doc from dream_runs where workspace_id = $1 and dream_id = $2
             order by seq desc",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }
}
