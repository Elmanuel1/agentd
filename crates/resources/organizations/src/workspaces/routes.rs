//! Axum routes for `/v1/organizations/workspaces`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::page::filters::bool_param;
use shared::{AdminContext, ApiJson, IdCursor, IdPage, ListSpec, Messages, RouteError, UnknownParams};

use crate::workspaces::requests::{CreateWorkspaceRequest, UpdateWorkspaceRequest};
use crate::workspaces::service::WorkspacesService;

/// The workspaces list: 1 to 1000 per page. agentd keeps no default
/// workspace, so `include_default` adds nothing.
const LIST: ListSpec = ListSpec {
    max_limit: 1000,
    filters: &["after_id", "before_id", "include_archived", "include_default"],
    unknown: UnknownParams::Ignore(&[]),
    messages: Messages::Files,
};

/// Build the `/v1/organizations/workspaces` router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/organizations/workspaces", post(create).get(list))
        .route("/v1/organizations/workspaces/{id}", get(get_one).post(update))
        .route("/v1/organizations/workspaces/{id}/archive", post(archive))
}

async fn create(
    Extension(svc): Extension<Arc<WorkspacesService>>,
    Extension(ctx): Extension<AdminContext>,
    ApiJson(body): ApiJson<CreateWorkspaceRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<WorkspacesService>>,
    Extension(ctx): Extension<AdminContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let limit = LIST.parse(&params)?.limit;
    bool_param(&params, "include_default")?;
    let include_archived = bool_param(&params, "include_archived")?;
    let cursor = IdCursor {
        after_id: params.get("after_id").cloned(),
        after_created: None,
        before_id: params.get("before_id").cloned(),
        limit,
    };
    let (rows, has_more) = svc.list(&ctx, include_archived, &cursor).await?;
    let page = IdPage::from_page(rows, has_more, |w| w.id.as_str());
    Ok(Json(page).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<WorkspacesService>>,
    Extension(ctx): Extension<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<WorkspacesService>>,
    Extension(ctx): Extension<AdminContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateWorkspaceRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update(&ctx, &id, body).await?).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<WorkspacesService>>,
    Extension(ctx): Extension<AdminContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive(&ctx, &id).await?).into_response())
}
