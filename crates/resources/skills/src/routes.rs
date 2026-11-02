//! Axum routes for `/v1/skills` — the Anthropic Skills API (multipart zip
//! upload, metadata, versions, delete). Beta header `skills-2025-10-02`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Multipart, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::{
    multipart_error, ApiError, AuthContext, Deleted, ListSpec, Messages, RouteError, UnknownParams,
};

use crate::{SkillListPage, SkillsService};

/// Skills lists (live: skills/40-42): limit 1 to 1000, unknown params ignored.
const LIST: ListSpec = ListSpec {
    max_limit: 1000,
    filters: &[],
    unknown: UnknownParams::Ignore(&[]),
    messages: Messages::Skills,
};

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
    while let Some(field) = multipart.next_field().await.map_err(|e| multipart_error(&e))? {
        match field.name() {
            Some("display_title") => {
                display_title = Some(field.text().await.map_err(|e| multipart_error(&e))?)
            }
            _ if field.file_name().is_some() => {
                zip = Some(field.bytes().await.map_err(|e| multipart_error(&e))?)
            }
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
    let page = LIST.parse(&params)?;
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
    Ok(Json(Deleted::new(id, "skill_deleted")).into_response())
}

async fn list_versions(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    Ok(Json(SkillListPage::from_fetched(svc.versions(&ctx, &id, &page).await?, &page)).into_response())
}

async fn delete_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    svc.delete_version(&ctx, &id, &version).await?;
    Ok(Json(Deleted::new(version, "skill_version_deleted")).into_response())
}
