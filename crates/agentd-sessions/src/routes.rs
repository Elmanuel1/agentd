//! Axum routes for `/v1/sessions` + events + SSE stream.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use agentd_core::{
    reject_unknown_params, ApiError, ApiJson, AuthContext, Page, Pagination, RequestId,
};
use axum::extract::{Extension, Path, Query};
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::Stream;
use serde::Deserialize;

use crate::events::EventPayload;
use crate::service::SessionsService;
use crate::types::{CreateSessionRequest, UpdateSessionRequest};

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

/// `POST …/events` body: `{"events":[…]}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendEventsBody {
    /// The events to enqueue.
    #[serde(default)]
    pub events: Vec<EventPayload>,
}

/// Build the sessions router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/sessions", post(create).get(list))
        .route(
            "/v1/sessions/{id}",
            get(get_one).post(update).delete(delete),
        )
        .route("/v1/sessions/{id}/archive", post(archive))
        .route(
            "/v1/sessions/{id}/events",
            get(list_events).post(send_events),
        )
        .route("/v1/sessions/{id}/events/stream", get(stream_events))
        .route("/v1/sessions/{id}/threads", get(list_threads))
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
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["limit", "page"])?;
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list(&ctx).await?, &page)).into_response())
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

/// `GET /v1/sessions/{id}/threads` — multi-agent sub-threads. agentd runs
/// single-agent sessions, so this is always `{data:[]}` (matching the live
/// shape for non-multiagent sessions); 404s if the session is unknown.
async fn list_threads(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["limit", "page"])?;
    svc.get(&ctx, &id).await?;
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(Vec::<serde_json::Value>::new(), &page)).into_response())
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
    Ok(Json(svc.delete(&ctx, &id).await?).into_response())
}

async fn list_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    reject_unknown_params(&params, &["limit", "page"])?;
    let page = Pagination::from_params(&params)?;
    Ok(Json(Page::paginate(svc.list_events(&ctx, &id).await?, &page)).into_response())
}

async fn send_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SendEventsBody>,
) -> Result<Response, RouteError> {
    Ok(Json(Page::of(svc.send_events(&ctx, &id, body.events).await?)).into_response())
}

async fn stream_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, RouteError> {
    use futures::StreamExt;
    let rx = svc.subscribe(&ctx, &id).await?;
    // Live opens the stream with a `: connected` comment frame before events.
    let connected =
        futures::stream::once(async { Ok(SseEvent::default().comment("connected")) });
    let events = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let data = serde_json::to_string(&ev).unwrap_or_default();
                    return Some((Ok(SseEvent::default().data(data)), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let stream = connected.chain(events);
    Ok(Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default()))
}
