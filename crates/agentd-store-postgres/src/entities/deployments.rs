//! `DeploymentsStore` on Postgres — native exactly-once claim via
//! `INSERT … ON CONFLICT DO NOTHING`.

use agentd_core::WorkspaceId;
use agentd_deployments::store::{DeploymentsStore, StoreError};
use agentd_deployments::types::{Deployment, DeploymentId, DeploymentRun};
use async_trait::async_trait;
use sqlx::types::Json;

use crate::{sqlx_msg, PgBackend};

/// Postgres-backed deployments store.
#[derive(Debug, Clone)]
pub struct PgDeploymentsStore {
    backend: PgBackend,
}

impl PgDeploymentsStore {
    /// Wrap a pool.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn be(e: sqlx::Error) -> StoreError {
    StoreError::Backend(sqlx_msg(e))
}

fn status_str(d: &Deployment) -> &'static str {
    use agentd_deployments::types::DeploymentStatus::*;
    match d.status {
        Active => "active",
        Paused => "paused",
        Archived => "archived",
    }
}

#[async_trait]
impl DeploymentsStore for PgDeploymentsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        sqlx::query(
            "insert into deployments (workspace_id, deployment_id, name, status, archived_at, created_at, doc)
             values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&ws.0)
        .bind(d.id.as_str())
        .bind(&d.name)
        .bind(status_str(&d))
        .bind(d.archived_at.map(|t| t.as_datetime()))
        .bind(d.created_at.as_datetime())
        .bind(Json(&d))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError> {
        let row: Option<(Json<Deployment>,)> = sqlx::query_as(
            "select doc from deployments where workspace_id = $1 and deployment_id = $2",
        )
        .bind(&ws.0)
        .bind(id.as_str())
        .fetch_optional(&self.backend.pool)
        .await
        .map_err(be)?;
        row.map(|(d,)| d.0).ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        let affected = sqlx::query(
            "update deployments set name = $3, status = $4, archived_at = $5, doc = $6
             where workspace_id = $1 and deployment_id = $2",
        )
        .bind(&ws.0)
        .bind(d.id.as_str())
        .bind(&d.name)
        .bind(status_str(&d))
        .bind(d.archived_at.map(|t| t.as_datetime()))
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

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Deployment>, StoreError> {
        let rows: Vec<(Json<Deployment>,)> = sqlx::query_as(
            "select doc from deployments where workspace_id = $1 order by created_at desc",
        )
        .bind(&ws.0)
        .fetch_all(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn count(&self, ws: &WorkspaceId) -> Result<usize, StoreError> {
        // A dashboard tile ("N deployments") is a single indexed count — not
        // an app-side scan.
        let (n,): (i64,) =
            sqlx::query_as("select count(*) from deployments where workspace_id = $1")
                .bind(&ws.0)
                .fetch_one(&self.backend.pool)
                .await
                .map_err(be)?;
        Ok(n.max(0) as usize)
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DeploymentRun) -> Result<(), StoreError> {
        sqlx::query(
            "insert into deployment_runs (workspace_id, deployment_id, run_id, created_at, has_error, doc)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&ws.0)
        .bind(run.deployment_id.as_str())
        .bind(run.id.as_str())
        .bind(run.created_at.as_datetime())
        .bind(run.error.is_some())
        .bind(Json(&run))
        .execute(&self.backend.pool)
        .await
        .map_err(be)?;
        Ok(())
    }

    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError> {
        // Filter pushed into SQL — dashboards ask "just the failures."
        let rows: Vec<(Json<DeploymentRun>,)> = match has_error {
            Some(want) => sqlx::query_as(
                "select doc from deployment_runs
                 where workspace_id = $1 and deployment_id = $2 and has_error = $3
                 order by created_at desc",
            )
            .bind(&ws.0)
            .bind(deployment.as_str())
            .bind(want)
            .fetch_all(&self.backend.pool)
            .await
            .map_err(be)?,
            None => sqlx::query_as(
                "select doc from deployment_runs
                 where workspace_id = $1 and deployment_id = $2 order by created_at desc",
            )
            .bind(&ws.0)
            .bind(deployment.as_str())
            .fetch_all(&self.backend.pool)
            .await
            .map_err(be)?,
        };
        Ok(rows.into_iter().map(|(d,)| d.0).collect())
    }

    async fn claim_fire(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        scheduled_at: &str,
    ) -> Result<bool, StoreError> {
        // The primary-key conflict is the exactly-once boundary: exactly one
        // replica's insert affects a row; the rest get zero.
        let affected = sqlx::query(
            "insert into deployment_claims (workspace_id, deployment_id, scheduled_at)
             values ($1, $2, $3) on conflict do nothing",
        )
        .bind(&ws.0)
        .bind(deployment.as_str())
        .bind(scheduled_at)
        .execute(&self.backend.pool)
        .await
        .map_err(be)?
        .rows_affected();
        Ok(affected == 1)
    }
}
