//! `/v1/sessions/{id}/events` and the SSE stream.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Extension, Path};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use shared::error::DataEnvelope;
use shared::page::filters::CreatedRange;
use shared::{
    ApiJson, AuthContext, ListQuery, ListSpec, Messages, Page, RouteError, SessionCaller, UnknownParams,
};
use tokio::sync::broadcast::error::RecvError;

use super::{environment, order, Worker};
use crate::events::{Event, EventPayload, SessionEvent};
use crate::service::SessionsService;
use crate::store::{EventFilter, Order};

/// `POST …/events` body: `{"events":[…]}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendEventsBody {
    /// The events to enqueue.
    #[serde(default)]
    pub events: Vec<EventPayload>,
}

/// The events list. Its limit goes to 1000: the SDK session tool runners
/// reconcile history with `limit=1000`.
const LIST: ListSpec = ListSpec {
    max_limit: 1000,
    filters: &[
        "created_at[gt]",
        "created_at[gte]",
        "created_at[lt]",
        "created_at[lte]",
        "order",
        "types",
        "types[]",
    ],
    unknown: UnknownParams::Reject,
    messages: Messages::ManagedAgents,
};

pub(super) fn router() -> Router {
    Router::new()
        .route("/v1/sessions/{id}/events", get(list_events).post(send_events))
        .route("/v1/sessions/{id}/events/stream", get(stream_events))
}

async fn list_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    worker: Worker,
    Path(id): Path<String>,
    query: ListQuery,
) -> Result<Response, RouteError> {
    if let Some(env) = environment(&worker) {
        svc.get(&ctx, &id).await?.seen_from(Some(env))?;
    }
    let params = query.map();
    let page = LIST.parse(&params)?;
    let filter = EventFilter {
        processed: CreatedRange::from_query(&params)?,
        order: order(params.get("order").map(String::as_str), Order::Asc)?,
        types: query.all("types").into_iter().map(str::to_owned).collect(),
        thread: None,
    };
    Ok(Json(Page::from_fetched(svc.list_events(&ctx, &id, &filter, &page).await?, &page)).into_response())
}

async fn send_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    worker: Worker,
    token: Option<Extension<SessionCaller>>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<SendEventsBody>,
) -> Result<Response, RouteError> {
    if token.is_some() {
        svc.guard_worker_results(&ctx.workspace, &id, &body.events).await?;
    }
    let sent = svc.send_events(&ctx, &id, environment(&worker), body.events).await?;
    Ok(Json(DataEnvelope { data: sent }).into_response())
}

/// One SSE frame per event, named by its `type` (both SDKs skip frames with
/// no `event:` line). The stream ends after `session.deleted`, and when this
/// subscriber falls behind the broadcast, so the client reconnects and
/// dedupes from the events list instead of silently missing events.
/// `event_deltas[]` previews are optional and not produced.
async fn stream_events(
    Extension(svc): Extension<Arc<SessionsService>>,
    Extension(ctx): Extension<AuthContext>,
    worker: Worker,
    Path(id): Path<String>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, RouteError> {
    let sub = svc.subscribe(&ctx, &id, environment(&worker)).await?;
    // Live opens the stream with a `: connected` comment frame before events.
    let connected = stream::once(async { Ok(SseEvent::default().comment("connected")) });
    let events = stream::unfold((sub, false), |(mut rx, done)| async move {
        if done {
            return None;
        }
        loop {
            match rx.recv().await {
                Ok(ev) if ev.on_session_stream() => {
                    let last = matches!(ev.payload, EventPayload::Session(SessionEvent::Deleted {}));
                    if let Some(frame) = frame(&ev) {
                        return Some((Ok(frame), (rx, last)));
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_) | RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(connected.chain(events)).keep_alive(KeepAlive::default()))
}

fn frame(ev: &Event) -> Option<SseEvent> {
    let data = serde_json::to_value(ev).ok()?;
    let name = data.get("type")?.as_str()?.to_owned();
    Some(SseEvent::default().event(name).data(data.to_string()))
}
