//! Axum routes for `/v1/agents` — thin: parse → service → respond.
//!
//! The router is mounted by `agentd-api`, which injects [`AuthContext`] and
//! the request-id/error-envelope middleware. Handlers here return
//! `Result<Json, ApiError>`; the envelope wrapping happens in one place.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, Pagination, RequestId};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::AgentsService;
use crate::types::{CreateAgentRequest, UpdateAgentRequest};

/// Wrapper so `ApiError` can be returned from handlers directly.
#[derive(Debug)]
pub struct RouteError(pub ApiError);

impl From<ApiError> for RouteError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        // Each response carries a fresh req_ id, like the live API.
        let rid = RequestId::generate();
        let body = self.0.to_envelope(&rid);
        let status = axum::http::StatusCode::from_u16(self.0.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}

/// Build the `/v1/agents` router. State is injected via Extensions so the
/// composing crate (`agentd-api`) owns middleware ordering.
pub fn router() -> Router {
    Router::new()
        .route("/v1/agents", post(create).get(list))
        .route("/v1/agents/{id}", get(get_one).post(update))
        .route("/v1/agents/{id}/archive", post(archive))
        .route("/v1/agents/{id}/versions", get(versions))
}

async fn create(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateAgentRequest>,
) -> Result<Response, RouteError> {
    let agent = svc.create(&ctx, body).await?;
    Ok(Json(agent).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_params(&params)?;
    let agents = svc.list(&ctx).await?;
    Ok(Json(Page::paginate(agents, &page)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let agent = svc.get(&ctx, &id).await?;
    Ok(Json(agent).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAgentRequest>,
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
    let page = Pagination::from_params(&params)?;
    let list = svc.versions(&ctx, &id).await?;
    Ok(Json(Page::paginate(list, &page)).into_response())
}
