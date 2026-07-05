//! Axum routes for `/v1/deployments`, `/v1/deployment_runs`, and the
//! scheduler-facing `/internal/fire`.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{
    reject_unknown_params, ApiError, AuthContext, Page, Pagination, RequestId, Timestamp,
};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::service::DeploymentsService;
use crate::types::CreateDeploymentRequest;

/// Error wrapper.
#[derive(Debug)]
pub struct RouteError(pub ApiError);

impl From<ApiError> for RouteError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        let rid = RequestId::generate();
        let body = self.0.to_envelope(&rid);
        let status = axum::http::StatusCode::from_u16(self.0.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}

/// `/internal/fire` body — the fixed scheduler contract.
#[derive(Debug, Deserialize)]
pub struct FireBody {
    /// `deployment/<id>` or `dream/<id>` target key… here: the deployment id.
    pub deployment_id: String,
    /// The tick instant (idempotency key with the id).
    pub scheduled_at: Timestamp,
}

/// Build the deployments router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/deployments", post(create).get(list))
        .route("/v1/deployments/{id}", get(get_one))
        .route("/v1/deployments/{id}/pause", post(pause))
        .route("/v1/deployments/{id}/unpause", post(unpause))
        .route("/v1/deployments/{id}/archive", post(archive))
        .route("/v1/deployments/{id}/run", post(run_manual))
        .route("/v1/deployment_runs", get(list_runs))
        .route("/internal/fire", post(internal_fire))
}

async fn create(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateDeploymentRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["limit", "page"])?;
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list(&ctx).await?, &page)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?).into_response())
}

async fn pause(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.pause(&ctx, &id).await?).into_response())
}

async fn unpause(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.unpause(&ctx, &id).await?).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive(&ctx, &id).await?).into_response())
}

async fn run_manual(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.run_manual(&ctx, &id).await?).into_response())
}

async fn list_runs(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["deployment_id", "has_error", "limit", "page"])?;
    let deployment_id = params
        .get("deployment_id")
        .cloned()
        .ok_or_else(|| ApiError::invalid_request("deployment_id: Field required"))?;
    let has_error = params
        .get("has_error")
        .and_then(|v| v.parse::<bool>().ok());
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(
        svc.runs(&ctx, &deployment_id, has_error).await?,
        &page,
    ))
    .into_response())
}

/// Scheduler → agentd. Business failures were already recorded as error
/// runs, so this returns 200 for them; only transport-level problems (bad
/// body, unknown id grammar) surface as errors — which the scheduler does
/// NOT retry (fire contract).
async fn internal_fire(
    Extension(svc): Extension<Arc<DeploymentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<FireBody>,
) -> Result<Response, RouteError> {
    let run = svc
        .fire_scheduled(&ctx, &body.deployment_id, body.scheduled_at)
        .await?;
    Ok(Json(serde_json::json!({ "fired": run.is_some(), "run": run })).into_response())
}
