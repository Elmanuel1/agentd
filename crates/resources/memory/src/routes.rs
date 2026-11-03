//! Axum routes for memory stores/memories/versions.
//!
//! Method table matters here: the live API answers `PATCH`/`PUT` on a memory
//! with **405 `Allow: DELETE, GET, HEAD, POST`** — axum's `MethodRouter`
//! produces exactly that when only get/post/delete are registered.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::page::filters::{bool_param, CreatedRange};
use shared::{reject_unknown_params, ApiError, ApiJson, AuthContext, Deleted, ListSpec, Page, RouteError};

use crate::service::{MemoriesService, MemoryListQuery, View};
use crate::store::StoreFilter;
use crate::types::{CreateMemoryRequest, CreateStoreRequest, UpdateMemoryRequest, UpdateStoreRequest};

mod versions;

pub(crate) fn view_of(params: &HashMap<String, String>, default: View) -> View {
    match params.get("view").map(String::as_str) {
        Some("full") => View::Full,
        Some("basic") => View::Basic,
        _ => default,
    }
}

/// The memory-stores list.
const LIST: ListSpec = ListSpec::managed(&["include_archived", "created_at[gte]", "created_at[lte]"]);
/// The memories list.
const MEMORIES: ListSpec = ListSpec::managed(&["path_prefix", "order_by", "depth", "view"]);

/// Build the memory router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/memory_stores", post(create_store).get(list_stores))
        .route("/v1/memory_stores/{id}", get(get_store).post(update_store).delete(delete_store))
        .route("/v1/memory_stores/{id}/archive", post(archive_store))
        .route("/v1/memory_stores/{id}/memories", post(create_memory).get(list_memories))
        // ONLY get/post/delete — PATCH/PUT get 405 + Allow, matching live.
        .route(
            "/v1/memory_stores/{id}/memories/{mid}",
            get(get_memory).post(update_memory).delete(delete_memory),
        )
        .merge(versions::router())
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
    let page = LIST.parse(&params)?;
    let filter = StoreFilter {
        include_archived: bool_param(&params, "include_archived")?,
        created: CreatedRange::from_query(&params)?,
    };
    Ok(Json(Page::from_fetched(svc.list_stores(&ctx, &filter, &page).await?, &page)).into_response())
}

async fn update_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateStoreRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update_store(&ctx, &id, body).await?).into_response())
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
    let store = svc.delete_store(&ctx, &id).await?;
    Ok(Json(Deleted::new(store.id.as_str(), "memory_store_deleted")).into_response())
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
    let mut page = MEMORIES.parse(&params)?;
    let view = view_of(&params, View::Basic);
    if view == View::Full {
        // Live: with `view=full`, a page holds at most 20 items.
        page.limit = page.limit.min(20);
    }
    let depth = match params.get("depth") {
        None => 0,
        Some(d) => d
            .parse::<u32>()
            .map_err(|_| ApiError::invalid_request("depth: Input should be a valid integer"))?,
    };
    let prefix = params.get("path_prefix").map(String::as_str);
    let order_by = params.get("order_by").map(String::as_str);
    let query = MemoryListQuery { path_prefix: prefix, order_by, depth, view };
    Ok(Json(svc.list_memories(&ctx, &id, &query, &page).await?).into_response())
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
    let memory = svc.delete_memory(&ctx, &id, &mid, expected).await?;
    Ok(Json(Deleted::new(memory.id.as_str(), "memory_deleted")).into_response())
}
