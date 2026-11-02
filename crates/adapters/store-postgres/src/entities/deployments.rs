//! `DeploymentsStore` on Postgres via Diesel — native exactly-once claim via
//! `INSERT … ON CONFLICT DO NOTHING`.

use ::deployments::store::{DeploymentsStore, StoreError};
use ::deployments::types::{Deployment, DeploymentId, DeploymentRun};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::{Pagination, WorkspaceId};

use crate::schema::{deployment_claims, deployment_runs, deployments};
use crate::{diesel_be, parse_doc, pool_be, to_doc, PgBackend};

/// Postgres-backed deployments store (Diesel).
#[derive(Debug, Clone)]
pub struct PgDeploymentsStore {
    backend: PgBackend,
}

impl PgDeploymentsStore {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn status_str(d: &Deployment) -> &'static str {
    use ::deployments::types::DeploymentStatus::*;
    match d.status {
        Active => "active",
        Paused => "paused",
        Archived => "archived",
    }
}

#[derive(diesel::Insertable)]
#[diesel(table_name = deployments)]
struct NewDeployment {
    workspace_id: String,
    deployment_id: String,
    name: String,
    status: String,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    doc: serde_json::Value,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = deployment_runs)]
struct NewRun {
    workspace_id: String,
    deployment_id: String,
    run_id: String,
    created_at: DateTime<Utc>,
    has_error: bool,
    doc: serde_json::Value,
}

#[derive(diesel::Insertable)]
#[diesel(table_name = deployment_claims)]
struct NewClaim {
    workspace_id: String,
    deployment_id: String,
    scheduled_at: String,
}

#[async_trait]
impl DeploymentsStore for PgDeploymentsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(deployments::table)
            .values(NewDeployment {
                workspace_id: ws.as_str().to_owned(),
                deployment_id: d.id.as_str().to_owned(),
                name: d.name.clone(),
                status: status_str(&d).to_owned(),
                archived_at: d.archived_at.map(|t| t.as_datetime()),
                created_at: d.created_at.as_datetime(),
                doc: to_doc(&d)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let doc: Option<serde_json::Value> = deployments::table
            .filter(deployments::workspace_id.eq(ws.as_str()))
            .filter(deployments::deployment_id.eq(id.as_str()))
            .select(deployments::doc)
            .first(&mut conn)
            .await
            .optional()
            .map_err(diesel_be)?;
        parse_doc(doc.ok_or(StoreError::NotFound)?)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let affected = diesel::update(
            deployments::table
                .filter(deployments::workspace_id.eq(ws.as_str()))
                .filter(deployments::deployment_id.eq(d.id.as_str())),
        )
        .set((
            deployments::name.eq(&d.name),
            deployments::status.eq(status_str(&d)),
            deployments::archived_at.eq(d.archived_at.map(|t| t.as_datetime())),
            deployments::doc.eq(to_doc(&d)?),
        ))
        .execute(&mut conn)
        .await
        .map_err(diesel_be)?;
        if affected == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Deployment>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let docs: Vec<serde_json::Value> = deployments::table
            .filter(deployments::workspace_id.eq(ws.as_str()))
            .order(deployments::created_at.desc())
            .select(deployments::doc)
            .limit(page.fetch_limit())
            .offset(page.sql_offset())
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn count(&self, ws: &WorkspaceId) -> Result<usize, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let n: i64 = deployments::table
            .filter(deployments::workspace_id.eq(ws.as_str()))
            .count()
            .get_result(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(n.max(0) as usize)
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DeploymentRun) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        diesel::insert_into(deployment_runs::table)
            .values(NewRun {
                workspace_id: ws.as_str().to_owned(),
                deployment_id: run.deployment_id.as_str().to_owned(),
                run_id: run.id.as_str().to_owned(),
                created_at: run.created_at.as_datetime(),
                has_error: run.error.is_some(),
                doc: to_doc(&run)?,
            })
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(())
    }

    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        let mut q = deployment_runs::table
            .filter(deployment_runs::workspace_id.eq(ws.as_str()))
            .filter(deployment_runs::deployment_id.eq(deployment.as_str()))
            .into_boxed();
        if let Some(want) = has_error {
            q = q.filter(deployment_runs::has_error.eq(want));
        }
        let docs: Vec<serde_json::Value> = q
            .order(deployment_runs::created_at.desc())
            .select(deployment_runs::doc)
            .load(&mut conn)
            .await
            .map_err(diesel_be)?;
        docs.into_iter().map(parse_doc).collect()
    }

    async fn claim_fire(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        scheduled_at: &str,
    ) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // The PK conflict is the exactly-once boundary: exactly one replica's
        // insert affects a row; the rest do nothing.
        let affected = diesel::insert_into(deployment_claims::table)
            .values(NewClaim {
                workspace_id: ws.as_str().to_owned(),
                deployment_id: deployment.as_str().to_owned(),
                scheduled_at: scheduled_at.to_owned(),
            })
            .on_conflict_do_nothing()
            .execute(&mut conn)
            .await
            .map_err(diesel_be)?;
        Ok(affected == 1)
    }
}
