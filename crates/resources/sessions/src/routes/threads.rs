//! `/v1/sessions/{id}/threads`: the primary thread and each thread a
//! coordinator started, with their own events and streams.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, Path, Query};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{self, Stream};
use futures::StreamExt;
use shared::page::filters::CreatedRange;
use shared::{AuthContext, ListQuery, ListSpec, Page, RouteError};
use tokio::sync::broadcast::error::RecvError;

use super::order;
use crate::service::SessionsService;
use crate::store::{EventFilter, Order};

/// The threads list.
const LIST: ListSpec = ListSpec::managed(&["statuses", "statuses[]"]);

/// A thread's events list.
const EVENTS: ListSpec = ListSpec::managed(&[
    "created_at[gt]",
    "created_at[gte]",
    "created_at[lt]",
    "created_at[lte]",
    "order",
    "types",
    "types[]",
]);

pub(super) fn router() -> Router {
    Router::new()
        .route("/v1/sessions/{id}/threads", get(list))
        .route("/v1/sessions/{id}/threads/{tid}", get(get_one))
        .route("/v1/sessions/{id}/threads/{tid}/archive", post(archive))
        .route("/v1/sessions/{id}/threads/{tid}/events", get(events))
        .route("/v1/sessions/{id}/threads/{tid}/stream", get(stream_events))
}

async fn list(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    Ok(Json(Page::paginate(svc.threads(&ctx, &id).await?, &page)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, tid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.thread(&ctx, &id, &tid).await?).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, tid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive_thread(&ctx, &id, &tid).await?).into_response())
}

async fn events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, tid)): Path<(String, String)>,
    query: ListQuery,
) -> Result<Response, RouteError> {
    let params = query.map();
    let page = EVENTS.parse(&params)?;
    let filter = EventFilter {
        processed: CreatedRange::from_query(&params)?,
        order: order(params.get("order").map(String::as_str), Order::Asc)?,
        types: query.all("types").into_iter().map(str::to_owned).collect(),
        thread: svc.thread_scope(&ctx, &id, &tid).await?,
    };
    let events = svc.list_events(&ctx, &id, &filter, &page).await?;
    let wire: Vec<serde_json::Value> = events.iter().map(crate::events::Event::thread_wire).collect();
    Ok(Json(Page::from_fetched(wire, &page)).into_response())
}

/// The thread's events as they happen, framed like the session stream.
async fn stream_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, tid)): Path<(String, String)>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, RouteError> {
    let scope = svc.thread_scope(&ctx, &id, &tid).await?;
    let sub = svc.subscribe(&ctx, &id).await?;
    let connected = stream::once(async { Ok(SseEvent::default().comment("connected")) });
    let events = stream::unfold((sub, scope), |(mut rx, scope)| async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.on_thread_stream(scope.as_ref()) => {
                    let data = ev.thread_wire();
                    let name =
                        data.get("type").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
                    return Some((Ok(SseEvent::default().event(name).data(data.to_string())), (rx, scope)));
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_) | RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(connected.chain(events)).keep_alive(KeepAlive::default()))
}
