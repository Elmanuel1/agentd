//! Axum routes for `/v1/files` — multipart upload, metadata, download, delete.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, RequestId};
use axum::extract::{Extension, Multipart, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

use crate::FilesService;

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

/// Build the files router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/files", axum::routing::post(upload).get(list))
        .route("/v1/files/{id}", get(metadata).delete(delete))
        .route("/v1/files/{id}/content", get(download))
}

async fn upload(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    mut multipart: Multipart,
) -> Result<Response, RouteError> {
    let mut filename = None;
    let mut mime = None;
    let mut bytes = None;
    let mut scope_id = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("file") => {
                filename = field.file_name().map(str::to_owned);
                mime = field.content_type().map(str::to_owned);
                bytes = field.bytes().await.ok();
            }
            Some("scope_id") => {
                scope_id = field.text().await.ok();
            }
            _ => {}
        }
    }
    let meta = svc
        .upload(
            &ctx,
            filename,
            mime,
            bytes.unwrap_or_default(),
            scope_id,
        )
        .await?;
    Ok(Json(meta).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let scope = params.get("scope_id").map(String::as_str);
    Ok(Json(Page::of(svc.list(&ctx, scope).await?)).into_response())
}

async fn metadata(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.metadata(&ctx, &id).await?).into_response())
}

async fn download(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let (meta, bytes) = svc.download(&ctx, &id).await?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, meta.mime_type)],
        bytes,
    )
        .into_response())
}

async fn delete(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete(&ctx, &id).await?).into_response())
}
