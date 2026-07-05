//! Axum routes for `/v1/environments`.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{reject_unknown_params, ApiError, AuthContext, Page, Pagination, RequestId};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::EnvironmentsService;
use crate::types::{CreateEnvironmentRequest, UpdateEnvironmentRequest};

/// Error wrapper (same pattern as every entity crate).
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

/// Build the `/v1/environments` router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/environments", post(create).get(list))
        .route(
            "/v1/environments/{id}",
            get(get_one).post(update).delete(delete),
        )
        .route("/v1/environments/{id}/archive", post(archive))
}

async fn create(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateEnvironmentRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["limit", "page"])?;
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list(&ctx).await?, &page)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<UpdateEnvironmentRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update(&ctx, &id, body).await?).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive(&ctx, &id).await?).into_response())
}

async fn delete(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    // Live probe: DELETE returned 200 (docs claim 204). Follow reality.
    Ok(Json(svc.delete(&ctx, &id).await?).into_response())
}
