//! Axum routes for `/v1/skills`. The GA shapes are served unless the request
//! sends `skills-2025-10-02`, which selects the beta shapes.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::multipart::MultipartRejection;
use axum::extract::{Extension, Multipart, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::beta::{BetaSet, SKILLS};
use shared::{
    multipart_error, ApiError, AuthContext, Deleted, ListSpec, Messages, Page, Pagination, RouteError,
    UnknownParams,
};

use crate::bundle::UploadFile;
use crate::types::{beta_version_id, SkillListPage, SkillSource};
use crate::SkillsService;

/// Skills lists (live skills/40-42): limit 1 to 1000, unknown params ignored
/// (Terraform sends `?beta=true`).
const LIST: ListSpec = ListSpec {
    max_limit: 1000,
    filters: &["source"],
    unknown: UnknownParams::Ignore(&[]),
    messages: Messages::Skills,
};

/// Build the skills router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/skills", post(create).get(list))
        .route("/v1/skills/{id}", get(get_one).delete(delete_one))
        .route("/v1/skills/{id}/versions", get(list_versions).post(create_version))
        .route("/v1/skills/{id}/versions/{version}", get(get_version).delete(delete_version))
        .route("/v1/skills/{id}/versions/{version}/content", get(download_version))
}

/// Every file part (`files[]`, or a single zip) and the label field.
async fn upload(
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(Vec<UploadFile>, Option<String>), ApiError> {
    let mut multipart = multipart.map_err(|_| ApiError::invalid_request("files[]: Field required"))?;
    let (mut files, mut label) = (Vec::new(), None);
    while let Some(field) = multipart.next_field().await.map_err(|e| multipart_error(&e))? {
        match (field.name().map(str::to_owned), field.file_name().map(str::to_owned)) {
            (Some(n), _) if n == "display_name" || n == "display_title" => {
                label = Some(field.text().await.map_err(|e| multipart_error(&e))?);
            }
            (_, Some(path)) => files.push((path, field.bytes().await.map_err(|e| multipart_error(&e))?)),
            _ => {}
        }
    }
    Ok((files, label))
}

fn page_of(rows: Vec<serde_json::Value>, page: &Pagination, beta: bool) -> SkillListPage {
    let inner = Page::from_fetched(rows, page);
    SkillListPage {
        has_more: beta.then_some(inner.next_page.is_some()),
        data: inner.data,
        next_page: inner.next_page,
    }
}

async fn create(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, RouteError> {
    let (files, label) = upload(multipart).await?;
    Ok(Json(svc.create(&ctx, files, label).await?.wire(betas.has(SKILLS))).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    let source = match params.get("source").map(String::as_str) {
        None => None,
        Some("custom") => Some(SkillSource::Custom),
        Some("anthropic") => Some(SkillSource::Anthropic),
        Some(_) => return Err(ApiError::invalid_request("source must be one of custom, anthropic").into()),
    };
    let beta = betas.has(SKILLS);
    let rows = svc.list(&ctx, source, &page).await?.iter().map(|s| s.wire(beta)).collect();
    Ok(Json(page_of(rows, &page, beta)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?.wire(betas.has(SKILLS))).into_response())
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
    Extension(betas): Extension<BetaSet>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    let beta = betas.has(SKILLS);
    let rows = svc.versions(&ctx, &id, &page, beta).await?.iter().map(|v| v.wire(beta)).collect();
    Ok(Json(page_of(rows, &page, beta)).into_response())
}

async fn create_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Path(id): Path<String>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, RouteError> {
    let (files, _) = upload(multipart).await?;
    Ok(Json(svc.create_version(&ctx, &id, files).await?.wire(betas.has(SKILLS))).into_response())
}

async fn get_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.version(&ctx, &id, &version).await?.wire(betas.has(SKILLS))).into_response())
}

async fn download_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    let zip = svc.version_bundle(&ctx, &id, &version, betas.has(SKILLS)).await?;
    Ok(([(axum::http::header::CONTENT_TYPE, "application/zip")], zip).into_response())
}

async fn delete_version(
    Extension(svc): Extension<Arc<SkillsService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Path((id, version)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    let deleted = svc.delete_version(&ctx, &id, &version).await?;
    let id = if betas.has(SKILLS) { beta_version_id(&deleted) } else { deleted.as_str().to_owned() };
    Ok(Json(Deleted::new(id, "skill_version_deleted")).into_response())
}
