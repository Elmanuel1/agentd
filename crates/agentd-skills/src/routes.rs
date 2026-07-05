//! Axum routes for `/v1/skills`.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, Pagination, RequestId};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::{CreateSkillRequest, CreateVersionRequest, SkillsService};

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

/// Build the skills router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/skills", post(create).get(list))
        .route("/v1/skills/{id}", get(get_one).delete(delete_one))
        .route(
            "/v1/skills/{id}/versions",
            post(create_version).get(list_versions),
        )
        .route(
            "/v1/skills/{id}/versions/{v}",
            get(get_version).delete(delete_version),
        )
}

async fn create(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateSkillRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list(&ctx).await?, &page)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?).into_response())
}

async fn delete_one(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete(&ctx, &id).await?).into_response())
}

async fn create_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<CreateVersionRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_version(&ctx, &id, body).await?).into_response())
}

async fn list_versions(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list_versions(&ctx, &id).await?, &page)).into_response())
}

async fn get_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, v)): Path<(String, u64)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_version(&ctx, &id, v).await?).into_response())
}

async fn delete_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, v)): Path<(String, u64)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete_version(&ctx, &id, v).await?).into_response())
}
