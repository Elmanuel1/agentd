//! `DeploymentsStore` on Postgres via Diesel.

use ::deployments::store::{NativeDeploymentsStore, StoreError};
use ::deployments::types::{Deployment, DeploymentId, DeploymentRun};
use chrono::{DateTime, Utc};
use diesel::{AnyJsonExpressionMethods, ExpressionMethods, OptionalExtension, QueryDsl};
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::ports::ScheduleChange;
use shared::{Pagination, Timestamp, WorkspaceId};

use crate::schema::deployments;
use crate::{PgBackend, diesel_be, parse_doc, pool_be, to_doc};

mod runs;

use super::schedules::outbox;

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

impl NativeDeploymentsStore for PgDeploymentsStore {
    async fn insert(
        &self,
        ws: &WorkspaceId,
        d: Deployment,
        schedule: &ScheduleChange,
    ) -> Result<(), StoreError> {
        let row = NewDeployment {
            workspace_id: ws.as_str().to_owned(),
            deployment_id: d.id.as_str().to_owned(),
            name: d.name.clone(),
            status: status_str(&d).to_owned(),
            archived_at: d.archived_at.map(|t| t.as_datetime()),
            created_at: d.created_at.as_datetime(),
            doc: to_doc(&d)?,
        };
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        conn.transaction::<_, diesel::result::Error, _>(async move |c| {
            diesel::insert_into(deployments::table).values(row).execute(c).await?;
            outbox::record(c, std::slice::from_ref(schedule)).await
        })
        .await
        .map_err(diesel_be)
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

    async fn replace(
        &self,
        ws: &WorkspaceId,
        d: Deployment,
        read: Timestamp,
        schedule: Option<&ScheduleChange>,
    ) -> Result<(), StoreError> {
        let doc = to_doc(&d)?;
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let written = conn
            .transaction::<_, diesel::result::Error, _>(async move |c| {
                let affected = diesel::update(
                    deployments::table
                        .filter(deployments::workspace_id.eq(ws.as_str()))
                        .filter(deployments::deployment_id.eq(d.id.as_str()))
                        .filter(deployments::doc.retrieve_as_text("updated_at").eq(read.to_wire())),
                )
                .set((
                    deployments::name.eq(&d.name),
                    deployments::status.eq(status_str(&d)),
                    deployments::archived_at.eq(d.archived_at.map(|t| t.as_datetime())),
                    deployments::doc.eq(doc),
                ))
                .execute(c)
                .await?;
                if let Some(change) = schedule.filter(|_| affected > 0) {
                    outbox::record(c, std::slice::from_ref(change)).await?;
                }
                Ok(affected > 0)
            })
            .await
            .map_err(diesel_be)?;
        written.then_some(()).ok_or(StoreError::Stale)
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
        runs::push(&self.backend, ws, run).await
    }

    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError> {
        runs::list(&self.backend, ws, deployment, has_error).await
    }
}
