//! Axum routes for `/v1/environments`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::page::filters::bool_param;
use shared::{ApiJson, AuthContext, Deleted, ListSpec, Page, RouteError};

use crate::service::EnvironmentsService;
use crate::types::{CreateEnvironmentRequest, UpdateEnvironmentRequest};

/// The environments list.
const LIST: ListSpec = ListSpec::managed(&["include_archived"]);

/// Build the `/v1/environments` router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/environments", post(create).get(list))
        .route("/v1/environments/{id}", get(get_one).post(update).delete(delete))
        .route("/v1/environments/{id}/archive", post(archive))
}

async fn create(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    ApiJson(body): ApiJson<CreateEnvironmentRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    let include_archived = bool_param(&params, "include_archived")?;
    Ok(Json(Page::from_fetched(svc.list(&ctx, include_archived, &page).await?, &page)).into_response())
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
    ApiJson(body): ApiJson<UpdateEnvironmentRequest>,
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
    let env = svc.delete(&ctx, &id).await?;
    Ok(Json(Deleted::new(env.id.as_str(), "environment_deleted")).into_response())
}
