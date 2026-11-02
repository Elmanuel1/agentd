//! `/v1/environments/{id}/work`: the environments work API, plus the
//! `ops` routes a hosted session's worker takes sandbox operations from.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sandbox_protocol::Outcome;
use serde::Deserialize;
use shared::{ApiError, ApiJson, AuthContext, ListSpec, Page, RouteError};

use crate::service::{WorkScope, WorkService};

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
        .route("/v1/environments/{id}/work/{work_id}/ops", get(next_op))
        .route("/v1/environments/{id}/work/{work_id}/ops/{op}", post(complete_op))
}

type Scope = Option<Extension<WorkScope>>;

fn scope(s: &Scope) -> Option<&WorkScope> {
    s.as_ref().map(|Extension(s)| s)
}

/// A work token reaches its own work item only, never the environment's.
fn key_only(s: &Scope) -> Result<(), RouteError> {
    match s {
        Some(_) => Err(RouteError(ApiError::not_found("work not found"))),
        None => Ok(()),
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
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path(env): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    key_only(&s)?;
    let page = LIST.parse(&params)?;
    Ok(Json(Page::paginate(svc.list(&ctx, &env), &page)).into_response())
}

async fn poll(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path(env): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    key_only(&s)?;
    Ok(Json(svc.poll(&ctx, &env, block(&params)?).await).into_response())
}

async fn stats(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path(env): Path<String>,
) -> Result<Response, RouteError> {
    key_only(&s)?;
    Ok(Json(svc.stats(&ctx, &env)).into_response())
}

async fn retrieve(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.retrieve(&ctx, scope(&s), &env, &id)?).into_response())
}

/// `POST …/work/{id}` body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateBody {
    metadata: BTreeMap<String, Option<String>>,
}

async fn update(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id)): Path<(String, String)>,
    ApiJson(body): ApiJson<UpdateBody>,
) -> Result<Response, RouteError> {
    key_only(&s)?;
    Ok(Json(svc.update(&ctx, &env, &id, body.metadata)?).into_response())
}

async fn ack(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.ack(&ctx, scope(&s), &env, &id)?).into_response())
}

async fn heartbeat(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let ttl = match params.get("desired_ttl_seconds") {
        Some(raw) => Some(
            raw.parse::<u64>()
                .map_err(|_| ApiError::invalid_request("desired_ttl_seconds: must be a whole number"))?,
        ),
        None => None,
    };
    let expected = params.get("expected_last_heartbeat").map(String::as_str);
    Ok(Json(svc.heartbeat(&ctx, scope(&s), &env, &id, expected, ttl)?).into_response())
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
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, RouteError> {
    let force = if body.is_empty() {
        false
    } else {
        serde_json::from_slice::<StopBody>(&body).map_err(|e| ApiError::invalid_request(e.to_string()))?.force
    };
    Ok(Json(svc.stop(&ctx, scope(&s), &env, &id, force)?).into_response())
}

/// The next sandbox operation for this work's worker, or `null`.
async fn next_op(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let work = svc.retrieve(
        &ctx,
        scope(&s).ok_or_else(|| ApiError::not_found("work not found"))?.into(),
        &env,
        &id,
    )?;
    Ok(Json(svc.board().next(&work.id, block(&params)?).await).into_response())
}

/// The outcome of a sandbox operation.
async fn complete_op(
    Extension(svc): Extension<Arc<WorkService>>,
    Extension(ctx): Extension<AuthContext>,
    s: Scope,
    Path((env, id, op)): Path<(String, String, String)>,
    ApiJson(outcome): ApiJson<Outcome>,
) -> Result<Response, RouteError> {
    let work = svc.retrieve(
        &ctx,
        scope(&s).ok_or_else(|| ApiError::not_found("work not found"))?.into(),
        &env,
        &id,
    )?;
    if !svc.board().complete(&work.id, &op, outcome) {
        return Err(RouteError(ApiError::not_found("operation not found")));
    }
    Ok(Json(serde_json::json!({"type": "work_op_result"})).into_response())
}
