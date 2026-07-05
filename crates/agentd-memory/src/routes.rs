//! Axum routes for memory stores/memories/versions.
//!
//! Method table matters here: the live API answers `PATCH`/`PUT` on a memory
//! with **405 `Allow: DELETE, GET, HEAD, POST`** — axum's `MethodRouter`
//! produces exactly that when only get/post/delete are registered.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{
    reject_unknown_params, ApiError, ApiJson, AuthContext, Page, Pagination, RequestId,
};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::{MemoriesService, View};
use crate::types::{CreateMemoryRequest, CreateStoreRequest, UpdateMemoryRequest};

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

fn view_of(params: &HashMap<String, String>, default: View) -> View {
    match params.get("view").map(String::as_str) {
        Some("full") => View::Full,
        Some("basic") => View::Basic,
        _ => default,
    }
}

/// Build the memory router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/memory_stores", post(create_store).get(list_stores))
        .route(
            "/v1/memory_stores/{id}",
            get(get_store).delete(delete_store),
        )
        .route("/v1/memory_stores/{id}/archive", post(archive_store))
        .route(
            "/v1/memory_stores/{id}/memories",
            post(create_memory).get(list_memories),
        )
        // ONLY get/post/delete — PATCH/PUT get 405 + Allow, matching live.
        .route(
            "/v1/memory_stores/{id}/memories/{mid}",
            get(get_memory).post(update_memory).delete(delete_memory),
        )
        .route(
            "/v1/memory_stores/{id}/memory_versions",
            get(list_versions),
        )
        .route(
            "/v1/memory_stores/{id}/memory_versions/{vid}",
            get(get_version),
        )
        .route(
            "/v1/memory_stores/{id}/memory_versions/{vid}/redact",
            post(redact_version),
        )
}

async fn create_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    ApiJson(body): ApiJson<CreateStoreRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_store(&ctx, body).await?).into_response())
}

async fn list_stores(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["limit", "page"])?;
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list_stores(&ctx).await?, &page)).into_response())
}

async fn get_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_store(&ctx, &id).await?).into_response())
}

async fn archive_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive_store(&ctx, &id).await?).into_response())
}

async fn delete_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete_store(&ctx, &id).await?).into_response())
}

async fn create_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<CreateMemoryRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_memory(&ctx, &id, body).await?).into_response())
}

async fn list_memories(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["path_prefix", "order_by", "depth"])?;
    let prefix = params.get("path_prefix").map(String::as_str);
    let order_by = params.get("order_by").map(String::as_str);
    let depth = params.get("depth").and_then(|d| d.parse::<u32>().ok());
    Ok(Json(svc.list_memories(&ctx, &id, prefix, order_by, depth).await?).into_response())
}

async fn get_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, mid)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["view"])?;
    // Retrieve defaults to full view (content included).
    let view = view_of(&params, View::Full);
    Ok(Json(svc.get_memory(&ctx, &id, &mid, view).await?).into_response())
}

async fn update_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, mid)): Path<(String, String)>,
    ApiJson(body): ApiJson<UpdateMemoryRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update_memory(&ctx, &id, &mid, body).await?).into_response())
}

async fn delete_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, mid)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["expected_content_sha256"])?;
    let expected = params.get("expected_content_sha256").map(String::as_str);
    Ok(Json(svc.delete_memory(&ctx, &id, &mid, expected).await?).into_response())
}

async fn list_versions(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["view", "memory_id", "limit", "page"])?;
    let view = view_of(&params, View::Basic);
    let memory_id = params.get("memory_id").map(String::as_str);
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(
        svc.list_versions(&ctx, &id, memory_id, view).await?,
        &page,
    ))
    .into_response())
}

async fn get_version(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, vid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_version(&ctx, &id, &vid).await?).into_response())
}

async fn redact_version(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, vid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.redact_version(&ctx, &id, &vid).await?).into_response())
}
