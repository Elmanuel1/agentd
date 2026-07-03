//! Axum routes for memory stores/memories/versions.
//!
//! Method table matters here: the live API answers `PATCH`/`PUT` on a memory
//! with **405 `Allow: DELETE, GET, HEAD, POST`** — axum's `MethodRouter`
//! produces exactly that when only get/post/delete are registered.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, RequestId};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::{MemoriesService, View};
use crate::types::{CreateMemoryRequest, CreateStoreRequest, UpdateMemoryRequest};

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

fn view_of(params: &HashMap<String, String>, default: View) -> View {
    match params.get("view").map(String::as_str) {
        Some("full") => View::Full,
        Some("basic") => View::Basic,
        _ => default,
    }
}

/// Build the memory router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/memory_stores", post(create_store).get(list_stores))
        .route(
            "/v1/memory_stores/{id}",
            get(get_store).delete(delete_store),
        )
        .route("/v1/memory_stores/{id}/archive", post(archive_store))
        .route(
            "/v1/memory_stores/{id}/memories",
            post(create_memory).get(list_memories),
        )
        // ONLY get/post/delete — PATCH/PUT get 405 + Allow, matching live.
        .route(
            "/v1/memory_stores/{id}/memories/{mid}",
            get(get_memory).post(update_memory).delete(delete_memory),
        )
        .route(
            "/v1/memory_stores/{id}/memory_versions",
            get(list_versions),
        )
        .route(
            "/v1/memory_stores/{id}/memory_versions/{vid}",
            get(get_version),
        )
        .route(
            "/v1/memory_stores/{id}/memory_versions/{vid}/redact",
            post(redact_version),
        )
}

async fn create_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateStoreRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_store(&ctx, body).await?).into_response())
}

async fn list_stores(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Response, RouteError> {
    Ok(Json(Page::of(svc.list_stores(&ctx).await?)).into_response())
}

async fn get_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_store(&ctx, &id).await?).into_response())
}

async fn archive_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive_store(&ctx, &id).await?).into_response())
}

async fn delete_store(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete_store(&ctx, &id).await?).into_response())
}

async fn create_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<CreateMemoryRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_memory(&ctx, &id, body).await?).into_response())
}

async fn list_memories(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let view = view_of(&params, View::Basic);
    let prefix = params.get("path_prefix").map(String::as_str);
    Ok(Json(Page::of(
        svc.list_memories(&ctx, &id, prefix, view).await?,
    ))
    .into_response())
}

async fn get_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, mid)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    // Retrieve defaults to full view (content included).
    let view = view_of(&params, View::Full);
    Ok(Json(svc.get_memory(&ctx, &id, &mid, view).await?).into_response())
}

async fn update_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, mid)): Path<(String, String)>,
    Json(body): Json<UpdateMemoryRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update_memory(&ctx, &id, &mid, body).await?).into_response())
}

async fn delete_memory(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, mid)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let expected = params.get("expected_content_sha256").map(String::as_str);
    Ok(Json(svc.delete_memory(&ctx, &id, &mid, expected).await?).into_response())
}

async fn list_versions(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let view = view_of(&params, View::Basic);
    let memory_id = params.get("memory_id").map(String::as_str);
    Ok(Json(Page::of(
        svc.list_versions(&ctx, &id, memory_id, view).await?,
    ))
    .into_response())
}

async fn get_version(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, vid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_version(&ctx, &id, &vid).await?).into_response())
}

async fn redact_version(
    Extension(svc): Extension<Arc<MemoriesService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, vid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.redact_version(&ctx, &id, &vid).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryMemoryStore;
    use agentd_core::{OrgId, WorkspaceId};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let svc = Arc::new(MemoriesService::new(Arc::new(InMemoryMemoryStore::new())));
        let ctx = AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_01t".into()),
            api_key_id: "apikey_01t".into(),
        };
        router().layer(Extension(svc)).layer(Extension(ctx))
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, Option<String>, serde_json::Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(match body {
                Some(b) => Body::from(b.to_owned()),
                None => Body::empty(),
            })
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let allow = resp
            .headers()
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, allow, v)
    }

    #[tokio::test]
    async fn patch_memory_is_405_with_allow_header_like_live() {
        let app = app();
        let (_, _, s) = call(
            &app,
            "POST",
            "/v1/memory_stores",
            Some(r#"{"name":"m","description":"d"}"#),
        )
        .await;
        let sid = s["id"].as_str().unwrap();
        let (_, _, mem) = call(
            &app,
            "POST",
            &format!("/v1/memory_stores/{sid}/memories"),
            Some(r#"{"path":"/a.md","content":"one"}"#),
        )
        .await;
        let mid = mem["id"].as_str().unwrap();
        // Live: PATCH → 405, allow: DELETE, GET, HEAD, POST
        let (st, allow, _) = call(
            &app,
            "PATCH",
            &format!("/v1/memory_stores/{sid}/memories/{mid}"),
            Some(r#"{"content":"two"}"#),
        )
        .await;
        assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);
        let allow = allow.unwrap_or_default();
        for m in ["GET", "POST", "DELETE"] {
            assert!(allow.contains(m), "allow header missing {m}: {allow}");
        }
        // PUT likewise.
        let (st, _, _) = call(
            &app,
            "PUT",
            &format!("/v1/memory_stores/{sid}/memories/{mid}"),
            Some(r#"{"content":"two"}"#),
        )
        .await;
        assert_eq!(st, StatusCode::METHOD_NOT_ALLOWED);
        // POST is the real update path (live-verified).
        let (st, _, updated) = call(
            &app,
            "POST",
            &format!("/v1/memory_stores/{sid}/memories/{mid}"),
            Some(r#"{"content":"two"}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_ne!(updated["memory_version_id"], mem["memory_version_id"]);
    }

    #[tokio::test]
    async fn path_conflict_wire_shape_over_http() {
        let app = app();
        let (_, _, s) = call(
            &app,
            "POST",
            "/v1/memory_stores",
            Some(r#"{"name":"m"}"#),
        )
        .await;
        let sid = s["id"].as_str().unwrap();
        call(
            &app,
            "POST",
            &format!("/v1/memory_stores/{sid}/memories"),
            Some(r#"{"path":"/a.md","content":"one"}"#),
        )
        .await;
        let (st, _, err) = call(
            &app,
            "POST",
            &format!("/v1/memory_stores/{sid}/memories"),
            Some(r#"{"path":"/a.md","content":"dup"}"#),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["error"]["type"], "memory_path_conflict_error");
        assert_eq!(err["error"]["conflicting_path"], "/a.md");
        assert!(err["error"]["conflicting_memory_id"]
            .as_str()
            .unwrap()
            .starts_with("mem_01"));
    }

    #[tokio::test]
    async fn versions_list_and_full_view_query() {
        let app = app();
        let (_, _, s) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
        let sid = s["id"].as_str().unwrap();
        let (_, _, mem) = call(
            &app,
            "POST",
            &format!("/v1/memory_stores/{sid}/memories"),
            Some(r#"{"path":"/a.md","content":"one"}"#),
        )
        .await;
        let mid = mem["id"].as_str().unwrap();
        let (st, _, versions) = call(
            &app,
            "GET",
            &format!("/v1/memory_stores/{sid}/memory_versions?memory_id={mid}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(versions["data"].as_array().unwrap().len(), 1);
        assert!(versions["data"][0].get("content").is_none());
        // Full view on retrieve includes content.
        let (st, _, got) = call(
            &app,
            "GET",
            &format!("/v1/memory_stores/{sid}/memories/{mid}?view=full"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(got["content"], "one");
    }
}
