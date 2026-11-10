//! `/v1/environments/{id}/work`: the environments work API.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use shared::{ApiError, ApiJson, ListSpec, RouteError};

use crate::caller::WorkCaller;
use crate::lifecycle::RECLAIM;
use crate::service::WorkService;

/// The work list.
const LIST: ListSpec = ListSpec::managed(&[]);

pub(super) fn router() -> Router {
    Router::new()
        .route("/v1/environments/{id}/work", get(list))
        .route("/v1/environments/{id}/work/poll", get(poll))
        .route("/v1/environments/{id}/work/stats", get(stats))
        .route("/v1/environments/{id}/work/{work_id}", get(retrieve).post(update))
        .route("/v1/environments/{id}/work/{work_id}/ack", post(ack))
        .route("/v1/environments/{id}/work/{work_id}/heartbeat", post(heartbeat))
        .route("/v1/environments/{id}/work/{work_id}/stop", post(stop))
}

/// `reclaim_older_than_ms`: hand out again an item handed out this long ago
/// and not acknowledged; 5000 when absent, as on live.
fn reclaim(params: &HashMap<String, String>) -> Result<Duration, RouteError> {
    match params.get("reclaim_older_than_ms") {
        None => Ok(RECLAIM),
        Some(raw) => raw.parse::<u64>().map(Duration::from_millis).map_err(|_| {
            RouteError(ApiError::invalid_request(
                "reclaim_older_than_ms: Input should be a valid integer, unable to parse string as an integer",
            ))
        }),
    }
}

/// `block_ms`: 1 to 999, as live caps it; absent means do not wait.
fn block(params: &HashMap<String, String>) -> Result<Duration, RouteError> {
    match params.get("block_ms") {
        None => Ok(Duration::ZERO),
        Some(raw) => match raw.parse::<u64>() {
            Ok(ms @ 1..=999) => Ok(Duration::from_millis(ms)),
            _ => Err(RouteError(ApiError::invalid_request("block_ms: must be between 1 and 999"))),
        },
    }
}

async fn list(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path(env): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = LIST.parse(&params)?;
    Ok(Json(svc.list(&caller, &env, &page).await?).into_response())
}

async fn poll(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path(env): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    if !matches!(&caller, WorkCaller::Queue { queue, .. } if *queue == env) {
        return Err(RouteError(ApiError::not_found("work not found")));
    }
    Ok(Json(svc.poll(&caller, block(&params)?, reclaim(&params)?).await?).into_response())
}

async fn stats(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path(env): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.stats(&caller, &env).await?).into_response())
}

async fn retrieve(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path((env, id)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.retrieve(&caller, &env, &id).await?).into_response())
}

/// `POST …/work/{id}` body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    metadata: BTreeMap<String, Option<String>>,
}

async fn update(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path((env, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update(&caller, &env, &id, body.metadata).await?).into_response())
}

async fn ack(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path((env, id)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.ack(&caller, &env, &id).await?).into_response())
}

async fn heartbeat(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path((env, id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let ttl = match params.get("desired_ttl_seconds") {
        Some(raw) => Some(raw.parse::<i64>().map_err(|_| {
            ApiError::invalid_request(
                "desired_ttl_seconds: Input should be a valid integer, unable to parse string as an integer",
            )
        })?),
        None => None,
    };
    let expected = params.get("expected_last_heartbeat").map(String::as_str);
    Ok(Json(svc.heartbeat(&caller, &env, &id, expected, ttl).await?).into_response())
}

/// `POST …/work/{id}/stop` body.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopBody {
    #[serde(default)]
    force: bool,
}

async fn stop(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(caller): Extension<WorkCaller>,
    Path((env, id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, RouteError> {
    let force = if body.is_empty() {
        false
    } else {
        serde_json::from_slice::<StopBody>(&body).map_err(|e| ApiError::invalid_request(e.to_string()))?.force
    };
    Ok(Json(svc.stop(&caller, &env, &id, force).await?).into_response())
}
