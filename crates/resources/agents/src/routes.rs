//! Axum routes for `/v1/agents` — thin: parse → service → respond.
//!
//! The router is mounted by `api`, which injects [`AuthContext`] and
//! the request-id/error-envelope middleware. Handlers here return
//! `Result<Json, ApiError>`; the envelope wrapping happens in one place.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::{reject_unknown_params, ApiError, ApiJson, AuthContext, ListSpec, Page, RouteError};

use crate::service::AgentsService;
use crate::types::{CreateAgentRequest, UpdateAgentRequest};

/// Every list here reads `limit` and `page` only.
const LIST: ListSpec = ListSpec::managed(&[]);

/// Build the `/v1/agents` router. State is injected via Extensions so the
/// composing crate (`api`) owns middleware ordering.
pub fn router() -> Router {
    Router::new()
        .route("/v1/agents", post(create).get(list))
        // DELETE is registered so 405s advertise `Allow: DELETE, GET, HEAD,
        // POST` (live), but agents are NOT deletable — DELETE itself 404s.
        .route("/v1/agents/{id}", get(get_one).post(update).delete(delete_not_supported))
        .route("/v1/agents/{id}/archive", post(archive))
        .route("/v1/agents/{id}/versions", get(versions))
}

async fn create(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    ApiJson(body): ApiJson<CreateAgentRequest>,
) -> Result<Response, RouteError> {
    let agent = svc.create(&ctx, body).await?;
    Ok(Json(agent).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    let agents = svc.list(&ctx, &page).await?;
    Ok(Json(Page::from_fetched(agents, &page)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["version"])?;
    let version = params.get("version").and_then(|v| v.parse::<u64>().ok());
    let agent = svc.get(&ctx, &id, version).await?;
    Ok(Json(agent).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateAgentRequest>,
) -> Result<Response, RouteError> {
    let agent = svc.update(&ctx, &id, body).await?;
    Ok(Json(agent).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let agent = svc.archive(&ctx, &id).await?;
    Ok(Json(agent).into_response())
}

async fn versions(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    let list = svc.versions(&ctx, &id).await?;
    Ok(Json(Page::paginate(list, &page)).into_response())
}

/// Live: `DELETE /v1/agents/{id}` returns 404 (agents cannot be deleted; the
/// method exists only so 405 `Allow` headers advertise it).
async fn delete_not_supported() -> Result<Response, RouteError> {
    Err(RouteError(ApiError::not_found("not found")))
}
