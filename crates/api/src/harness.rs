//! What a session's harness, running in its sandbox, exchanges with agentd:
//! the events it emits, the client events it takes, the files it mounts (a
//! client's uploads are never downloadable, so only this session's own reach
//! it here), and the files it leaves under `/mnt/session/outputs`. Only the
//! session's own work token reaches these routes; to anything else the
//! session does not exist here.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use files::FilesService;
use serde::Deserialize;
use sessions::events::{Rubric, UserEvent};
use sessions::types::{SessionId, SessionResource, ThreadId};
use sessions::{EventPayload, HarnessContract, SessionsService};
use shared::{ApiError, ApiJson, AuthContext, Module, RouteError};
use work::WorkScope;

/// The services the harness routes write through.
#[derive(Debug, Clone)]
pub struct Harness {
    /// Sessions and their event log.
    pub sessions: Arc<SessionsService>,
    /// Session-scoped output files.
    pub files: Arc<FilesService>,
}

/// The harness routes.
pub fn module(harness: Harness) -> Module {
    let router = Router::new()
        .route("/v1/sessions/{id}/harness/events", post(events))
        .route("/v1/sessions/{id}/harness/take", post(take))
        .route("/v1/sessions/{id}/harness/outputs", put(output))
        .route("/v1/sessions/{id}/harness/files/{file_id}", get(file));
    Module::new(router, harness)
}

/// `id` when the caller is that session's harness.
fn own(scope: Option<Extension<WorkScope>>, id: &str) -> Result<SessionId, RouteError> {
    match scope {
        Some(Extension(WorkScope(claims))) if claims.session == id => {
            SessionId::parse(id).map_err(|_| RouteError(ApiError::not_found("session not found")))
        }
        _ => Err(RouteError(ApiError::not_found("session not found"))),
    }
}

/// One reported event: the wire payload and the thread it belongs to.
#[derive(Deserialize)]
struct Reported {
    #[serde(default)]
    session_thread_id: Option<ThreadId>,
    #[serde(flatten)]
    payload: EventPayload,
}

#[derive(Deserialize)]
struct Report {
    events: Vec<Reported>,
}

async fn events(
    Extension(h): Extension<Harness>,
    Extension(ctx): Extension<AuthContext>,
    scope: Option<Extension<WorkScope>>,
    Path(id): Path<String>,
    ApiJson(report): ApiJson<Report>,
) -> Result<Response, RouteError> {
    let session = own(scope, &id)?;
    let mut recorded = Vec::with_capacity(report.events.len());
    for e in report.events {
        let thread = e.session_thread_id.as_ref();
        recorded.push(h.sessions.record(&ctx.workspace, &session, thread, e.payload).await?);
    }
    Ok(Json(serde_json::json!({ "data": recorded })).into_response())
}

/// The client events waiting for the agent, stamped processed as they are taken.
async fn take(
    Extension(h): Extension<Harness>,
    Extension(ctx): Extension<AuthContext>,
    scope: Option<Extension<WorkScope>>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let session = own(scope, &id)?;
    let taken = h.sessions.drain_user_events(&ctx.workspace, &session).await?;
    Ok(Json(serde_json::json!({ "data": taken })).into_response())
}

/// Keep the body as the session's output at `path`, in place of `previous`.
async fn output(
    Extension(h): Extension<Harness>,
    Extension(ctx): Extension<AuthContext>,
    scope: Option<Extension<WorkScope>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, RouteError> {
    own(scope, &id)?;
    let path = params.get("path").filter(|p| !p.is_empty() && !p.contains("..")).ok_or_else(|| {
        RouteError(ApiError::invalid_request("path: a file under /mnt/session/outputs is required"))
    })?;
    let saved = h.files.save_output(&ctx, &id, path, body).await?;
    if let Some(previous) = params.get("previous").filter(|p| p.as_str() != saved.id.as_str()) {
        // The earlier version of this output, one file per path; never a file
        // that is not this session's own output.
        let own_output =
            h.files.metadata(&ctx, previous).await.is_ok_and(
                |f| matches!(&f.scope, Some(files::types::FileScope::Session { id: s }) if *s == id),
            );
        if own_output {
            let _ = h.files.delete(&ctx, previous).await;
        }
    }
    Ok(Json(saved).into_response())
}

/// The bytes of a file the session mounts, or a rubric its outcomes name.
async fn file(
    Extension(h): Extension<Harness>,
    Extension(ctx): Extension<AuthContext>,
    scope: Option<Extension<WorkScope>>,
    Path((id, file_id)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    let session = own(scope, &id)?;
    let mounted = h
        .sessions
        .get(&ctx, &id)
        .await?
        .resources
        .iter()
        .any(|r| matches!(r, SessionResource::File { file_id: f, .. } if *f == file_id));
    let rubric = || async {
        let events = h.sessions.events(&ctx.workspace, &session).await?;
        Ok::<_, ApiError>(events.iter().any(|e| {
            matches!(&e.payload, EventPayload::User(UserEvent::DefineOutcome { rubric: Rubric::File { file_id: f }, .. }) if *f == file_id)
        }))
    };
    if !mounted && !rubric().await? {
        return Err(RouteError(ApiError::not_found("file not found")));
    }
    let (_, bytes) = h.files.content(&ctx, &file_id).await?;
    Ok(([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}
