//! `/v1/memory_stores/{id}/memory_versions`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::page::filters::CreatedRange;
use shared::{ApiError, AuthContext, ListSpec, Page, RouteError};

use super::view_of;
use crate::service::{MemoriesService, View};
use crate::store::VersionFilter;

/// The memory-versions list.
const VERSIONS: ListSpec = ListSpec::managed(&[
    "view",
    "memory_id",
    "api_key_id",
    "session_id",
    "service_account_id",
    "operation",
    "created_at[gte]",
    "created_at[lte]",
]);

pub(super) fn router() -> Router {
    Router::new()
        .route("/v1/memory_stores/{id}/memory_versions", get(list_versions))
        .route("/v1/memory_stores/{id}/memory_versions/{vid}", get(get_version))
        .route("/v1/memory_stores/{id}/memory_versions/{vid}/redact", post(redact_version))
}

async fn list_versions(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let view = view_of(&params, View::Basic);
    let page = VERSIONS.parse(&params)?;
    let operation = params
        .get("operation")
        .map(|o| {
            serde_json::from_value(serde_json::Value::String(o.clone())).map_err(|_| {
                ApiError::invalid_request("operation: Input should be 'created', 'modified' or 'deleted'")
            })
        })
        .transpose()?;
    let filter = VersionFilter {
        memory_id: params.get("memory_id").cloned(),
        api_key_id: params.get("api_key_id").cloned(),
        session_id: params.get("session_id").cloned(),
        service_account_id: params.get("service_account_id").cloned(),
        operation,
        created: CreatedRange::from_query(&params)?,
    };
    let versions = svc.list_versions(&ctx, &id, &filter, view, &page).await?;
    Ok(Json(Page::from_fetched(versions, &page)).into_response())
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
