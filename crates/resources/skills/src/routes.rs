//! Axum routes for `/v1/skills` — the Anthropic Skills API (multipart zip
//! upload, metadata, versions, delete). Beta header `skills-2025-10-02`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Multipart, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::{ApiError, AuthContext, Pagination, RouteError};

use crate::{SkillListPage, SkillsService};

/// Build the skills router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/skills", post(create).get(list))
        .route("/v1/skills/{id}", get(get_one).delete(delete_one))
        .route("/v1/skills/{id}/versions", get(list_versions))
        .route("/v1/skills/{id}/versions/{version}", axum::routing::delete(delete_version))
}

async fn create(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    mut multipart: Multipart,
) -> Result<Response, RouteError> {
    let mut zip = None;
    let mut display_title = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("display_title") => display_title = field.text().await.ok(),
            _ if field.file_name().is_some() => zip = field.bytes().await.ok(),
            _ => {}
        }
    }
    let zip = zip.ok_or_else(|| ApiError::invalid_request("files: Field required"))?;
    Ok(Json(svc.create(&ctx, zip, display_title).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_list_params(&params)?;
    Ok(Json(SkillListPage::from_fetched(svc.list(&ctx, &page).await?, &page)).into_response())
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
    svc.delete(&ctx, &id).await?;
    Ok(StatusCode::OK.into_response())
}

async fn list_versions(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_list_params(&params)?;
    Ok(Json(SkillListPage::from_fetched(svc.versions(&ctx, &id, &page).await?, &page)).into_response())
}

async fn delete_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    svc.delete_version(&ctx, &id, &version).await?;
    Ok(StatusCode::OK.into_response())
}
