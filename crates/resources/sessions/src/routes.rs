//! Axum routes for `/v1/sessions`; events and the stream, resources and
//! threads each have a file under `routes/`.

use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use shared::page::filters::{bool_param, CreatedRange};
use shared::{ApiError, ApiJson, AuthContext, Deleted, ListQuery, ListSpec, Page, RouteError};

use crate::service::SessionsService;
use crate::store::{Order, SessionFilter};
use crate::types::{CreateSessionRequest, SessionStatus, UpdateSessionRequest};

mod events;
mod resources;
mod threads;

/// The sessions list.
const LIST: ListSpec = ListSpec::managed(&[
    "agent_id",
    "agent_version",
    "created_at[gt]",
    "created_at[gte]",
    "created_at[lt]",
    "created_at[lte]",
    "deployment_id",
    "include_archived",
    "memory_store_id",
    "order",
    "statuses",
    "statuses[]",
]);

/// Build the sessions router.
pub fn router() -> Router {
    let sessions = Router::new()
        .route("/v1/sessions", post(create).get(list))
        .route("/v1/sessions/{id}", get(get_one).post(update).delete(delete))
        .route("/v1/sessions/{id}/archive", post(archive));
    sessions.merge(events::router()).merge(resources::router()).merge(threads::router())
}

async fn create(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    ApiJson(body): ApiJson<CreateSessionRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    query: ListQuery,
) -> Result<Response, RouteError> {
    let params = query.map();
    let page = LIST.parse(&params)?;
    let filter = SessionFilter {
        agent_id: params.get("agent_id").cloned(),
        agent_version: params
            .get("agent_version")
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| ApiError::invalid_request("agent_version: Input should be a valid integer"))
            })
            .transpose()?,
        created: CreatedRange::from_query(&params)?,
        deployment_id: params.get("deployment_id").cloned(),
        include_archived: bool_param(&params, "include_archived")?,
        memory_store_id: params.get("memory_store_id").cloned(),
        order: order(params.get("order").map(String::as_str), Order::Desc)?,
        statuses: query.all("statuses").into_iter().map(status).collect::<Result<_, _>>()?,
    };
    let sessions = svc.list(&ctx, &filter, &page).await?;
    Ok(Json(Page::from_fetched(sessions, &page).with_prev_page(&page)).into_response())
}

/// `asc` or `desc`; `default` when left out.
pub(crate) fn order(raw: Option<&str>, default: Order) -> Result<Order, ApiError> {
    match raw {
        None => Ok(default),
        Some("asc") => Ok(Order::Asc),
        Some("desc") => Ok(Order::Desc),
        Some(_) => Err(ApiError::invalid_request("order: Input should be 'asc' or 'desc'")),
    }
}

fn status(raw: &str) -> Result<SessionStatus, ApiError> {
    serde_json::from_value(serde_json::Value::String(raw.to_owned())).map_err(|_| {
        ApiError::invalid_request(
            "statuses: Input should be 'rescheduling', 'running', 'idle' or 'terminated'",
        )
    })
}

async fn get_one(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UpdateSessionRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update(&ctx, &id, body).await?).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive(&ctx, &id).await?).into_response())
}

async fn delete(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let session = svc.delete(&ctx, &id).await?;
    Ok(Json(Deleted::new(session.id.as_str(), "session_deleted")).into_response())
}
