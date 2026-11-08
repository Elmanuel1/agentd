//! `/v1/sessions/{id}/threads`. agentd runs single-agent sessions, which have
//! no session threads: the list is empty, and any thread id is not found.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::{ApiError, AuthContext, ListSpec, Page, RouteError};

use crate::service::SessionsService;

/// The threads list.
const LIST: ListSpec = ListSpec::managed(&["statuses", "statuses[]"]);

pub(super) fn router() -> Router {
    Router::new()
        .route("/v1/sessions/{id}/threads", get(list))
        .route("/v1/sessions/{id}/threads/{tid}", get(not_found))
        .route("/v1/sessions/{id}/threads/{tid}/archive", post(not_found))
        .route("/v1/sessions/{id}/threads/{tid}/events", get(not_found))
        .route("/v1/sessions/{id}/threads/{tid}/stream", get(not_found))
}

async fn list(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    svc.get(&ctx, &id).await?;
    let page = LIST.parse(&params)?;
    Ok(Json(Page::paginate(Vec::<serde_json::Value>::new(), &page)).into_response())
}

/// The session must exist; the thread never does.
async fn not_found(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, _tid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    svc.get(&ctx, &id).await?;
    Err(RouteError(ApiError::not_found("session thread not found")))
}
