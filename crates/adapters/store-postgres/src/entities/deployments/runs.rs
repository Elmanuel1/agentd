//! A deployment's run records and the exactly-once claim of each fire.

use ::deployments::store::StoreError;
use ::deployments::types::{DeploymentId, DeploymentRun};
use chrono::{DateTime, Utc};
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::WorkspaceId;

use crate::schema::{deployment_claims, deployment_runs};
use crate::{diesel_be, parse_doc, pool_be, to_doc, PgBackend};

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

pub(super) async fn push(
    backend: &PgBackend,
    ws: &WorkspaceId,
    run: DeploymentRun,
) -> Result<(), StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
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

pub(super) async fn list(
    backend: &PgBackend,
    ws: &WorkspaceId,
    deployment: &DeploymentId,
    has_error: Option<bool>,
) -> Result<Vec<DeploymentRun>, StoreError> {
    let mut conn = backend.diesel.get().await.map_err(pool_be)?;
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

pub(super) async fn claim(
    backend: &PgBackend,
    ws: &WorkspaceId,
    deployment: &DeploymentId,
    scheduled_at: &str,
) -> Result<bool, StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
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
