//! `/v1/sessions/{id}/resources`.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use shared::{ApiJson, AuthContext, Deleted, ListSpec, Page, RouteError};

use crate::service::SessionsService;
use crate::types::SessionResourceInput;

/// The resources list reads `limit` and `page` only.
const LIST: ListSpec = ListSpec::managed(&[]);

/// `POST …/resources/{rid}` body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateResourceBody {
    /// The repository's new token; never stored or echoed.
    #[allow(dead_code)]
    pub authorization_token: String,
}

pub(super) fn router() -> Router {
    Router::new()
        .route("/v1/sessions/{id}/resources", get(list).post(add))
        .route("/v1/sessions/{id}/resources/{rid}", get(get_one).post(update).delete(delete))
}

async fn list(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    Ok(Json(Page::paginate(svc.resources(&ctx, &id).await?, &page)).into_response())
}

async fn add(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SessionResourceInput>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.add_resource(&ctx, &id, body).await?).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, rid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.resource(&ctx, &id, &rid).await?).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, rid)): Path<(String, String)>,
    ApiJson(_body): ApiJson<UpdateResourceBody>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update_resource(&ctx, &id, &rid).await?).into_response())
}

async fn delete(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, rid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    let rid = svc.delete_resource(&ctx, &id, &rid).await?;
    Ok(Json(Deleted::new(rid.as_str(), "session_resource_deleted")).into_response())
}
