//! Axum routes for `/v1/files`: multipart upload, metadata, download, delete.
//! The GA shapes are served unless the request sends `files-api-2025-04-14`,
//! which selects the beta shapes (live files/11, 13, 29).

use std::sync::Arc;

use axum::extract::multipart::MultipartRejection;
use axum::extract::{Extension, Multipart, Path};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shared::beta::{BetaSet, FILES_API};
use shared::Timestamp;
use shared::{
    multipart_error, ApiError, AuthContext, Deleted, IdCursor, IdPage, ListQuery, ListSpec, Messages,
    RouteError, SessionCaller, UnknownParams,
};

use crate::types::FileMetadata;
use crate::Mounts;
use crate::{FileFilter, FilesService};

/// The files list (live files/21-28): limit 1 to 1000, unknown params
/// ignored, the legacy cursor params rejected.
const LIST: ListSpec = ListSpec {
    max_limit: 1000,
    filters: &["ids", "ids[]", "scope_id"],
    unknown: UnknownParams::Ignore(&["after_id", "before_id"]),
    messages: Messages::Files,
};

/// The beta files list pages with `after_id` / `before_id`.
const BETA_LIST: ListSpec = ListSpec {
    max_limit: 1000,
    filters: &["after_id", "before_id", "scope_id", "ids", "ids[]"],
    unknown: UnknownParams::Ignore(&[]),
    messages: Messages::Files,
};

/// Set when the caller is a session's own work token.
type Caller = Option<Extension<SessionCaller>>;

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
    Extension(betas): Extension<BetaSet>,
    caller: Caller,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<Response, RouteError> {
    // Live (files/09): a body that is not multipart has no `file` field.
    let mut multipart = multipart.map_err(|_| ApiError::invalid_request("file: Field required"))?;
    let (mut filename, mut mime, mut bytes, mut scope_id, mut expires) = (None, None, None, None, None);
    while let Some(field) = multipart.next_field().await.map_err(|e| multipart_error(&e))? {
        match field.name() {
            Some("file") => {
                filename = field.file_name().map(str::to_owned);
                mime = field.content_type().map(str::to_owned);
                bytes = Some(field.bytes().await.map_err(|e| multipart_error(&e))?);
            }
            Some("scope_id") => scope_id = Some(field.text().await.map_err(|e| multipart_error(&e))?),
            Some("expires_in_seconds") => {
                expires = Some(field.text().await.map_err(|e| multipart_error(&e))?)
            }
            _ => {}
        }
    }
    let bytes = bytes.unwrap_or_default();
    let meta = match caller {
        Some(Extension(caller)) => svc.upload_for(&ctx, &caller, filename, bytes).await?,
        None => svc.upload(&ctx, filename, mime, bytes, scope_id, expires).await?,
    };
    Ok(Json(meta.wire(betas.has(FILES_API))).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    query: ListQuery,
) -> Result<Response, RouteError> {
    let mut params = query.map();
    let filter = FileFilter {
        scope_id: params.get("scope_id").cloned(),
        ids: query.all("ids").into_iter().map(str::to_owned).collect(),
    };
    if betas.has(FILES_API) {
        let limit = BETA_LIST.parse(&params)?.limit;
        let cursor = IdCursor {
            after_id: params.get("after_id").cloned(),
            after_created: None,
            before_id: params.get("before_id").cloned(),
            limit,
        };
        let (files, has_more) = svc.list(&ctx, &filter, &cursor).await?;
        let page = IdPage::from_page(files, has_more, |f| f.id.as_str());
        let data: Vec<_> = page.data.iter().map(|f| f.wire(true)).collect();
        return Ok(Json(serde_json::json!({
            "data": data, "first_id": page.first_id, "has_more": page.has_more, "last_id": page.last_id,
        }))
        .into_response());
    }
    // The GA cursor is the last file served, its creation time and id, opaque
    // to clients; it stays valid when that file is deleted.
    let after = params.remove("page").map(|p| decode(&p)).transpose()?;
    let limit = LIST.parse(&params)?.limit;
    let (after_created, after_id) = after.map_or((None, None), |(t, id)| (Some(t), Some(id)));
    let cursor = IdCursor { after_id, after_created, before_id: None, limit };
    let (files, has_more) = svc.list(&ctx, &filter, &cursor).await?;
    let next_page = has_more.then(|| files.last().map(encode)).flatten();
    let data: Vec<_> = files.iter().map(|f| f.wire(false)).collect();
    Ok(Json(serde_json::json!({"data": data, "next_page": next_page})).into_response())
}

fn encode(last: &FileMetadata) -> String {
    format!("page_{}", URL_SAFE_NO_PAD.encode(format!("{}|{}", last.created_at.to_wire(), last.id.as_str())))
}

fn decode(page: &str) -> Result<(Timestamp, String), ApiError> {
    let invalid = || ApiError::invalid_request("page: invalid cursor");
    let raw = page.strip_prefix("page_").and_then(|b| URL_SAFE_NO_PAD.decode(b).ok()).ok_or_else(invalid)?;
    let (created, id) = std::str::from_utf8(&raw).ok().and_then(|s| s.split_once('|')).ok_or_else(invalid)?;
    let created =
        serde_json::from_value(serde_json::Value::String(created.to_owned())).map_err(|_| invalid())?;
    Ok((created, id.to_owned()))
}

async fn metadata(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(betas): Extension<BetaSet>,
    Extension(mounts): Extension<Mounts>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let meta = match caller {
        Some(Extension(caller)) => svc.metadata_for(&ctx, &mounts, &caller, &id).await?,
        None => svc.metadata(&ctx, &id).await?,
    };
    Ok(Json(meta.wire(betas.has(FILES_API))).into_response())
}

async fn download(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    Extension(mounts): Extension<Mounts>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let (meta, bytes) = match caller {
        Some(Extension(caller)) => svc.download_for(&ctx, &mounts, &caller, &id).await?,
        None => svc.download(&ctx, &id).await?,
    };
    Ok(([(axum::http::header::CONTENT_TYPE, meta.mime_type)], bytes).into_response())
}

async fn delete(
    Extension(svc): Extension<Arc<FilesService>>,
    Extension(ctx): Extension<AuthContext>,
    caller: Caller,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let file = match caller {
        Some(Extension(caller)) => svc.delete_for(&ctx, &caller, &id).await?,
        None => svc.delete(&ctx, &id).await?,
    };
    Ok(Json(Deleted::new(file.id.as_str(), "file_deleted")).into_response())
}
