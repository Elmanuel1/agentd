//! Axum routes for `/v1/environments`.

use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, RequestId};
use axum::extract::{Extension, Path};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::EnvironmentsService;
use crate::types::{CreateEnvironmentRequest, UpdateEnvironmentRequest};

/// Error wrapper (same pattern as every entity crate).
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

/// Build the `/v1/environments` router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/environments", post(create).get(list))
        .route(
            "/v1/environments/{id}",
            get(get_one).post(update).delete(delete),
        )
        .route("/v1/environments/{id}/archive", post(archive))
}

async fn create(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateEnvironmentRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create(&ctx, body).await?).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Response, RouteError> {
    Ok(Json(Page::of(svc.list(&ctx).await?)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get(&ctx, &id).await?).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<UpdateEnvironmentRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update(&ctx, &id, body).await?).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive(&ctx, &id).await?).into_response())
}

async fn delete(
    Extension(svc): Extension<Arc<EnvironmentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    // Live probe: DELETE returned 200 (docs claim 204). Follow reality.
    Ok(Json(svc.delete(&ctx, &id).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryEnvironmentsStore;
    use agentd_core::{OrgId, WorkspaceId};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let svc = Arc::new(EnvironmentsService::new(Arc::new(
            InMemoryEnvironmentsStore::new(),
        )));
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
    ) -> (StatusCode, serde_json::Value) {
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
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    #[tokio::test]
    async fn create_get_delete_roundtrip() {
        let app = app();
        let (st, created) = call(
            &app,
            "POST",
            "/v1/environments",
            Some(r#"{"name":"compat-probe-env","config":{"type":"cloud","networking":{"type":"unrestricted"}}}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(created["scope"], "organization");
        let id = created["id"].as_str().unwrap();
        let (st, got) = call(&app, "GET", &format!("/v1/environments/{id}"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(got, created);
        let (st, _) = call(&app, "DELETE", &format!("/v1/environments/{id}"), None).await;
        assert_eq!(st, StatusCode::OK); // live behavior, not the documented 204
        let (st, _) = call(&app, "GET", &format!("/v1/environments/{id}"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn archive_route_sets_state() {
        let app = app();
        let (_, created) = call(
            &app,
            "POST",
            "/v1/environments",
            Some(r#"{"name":"e","config":{"type":"self_hosted"}}"#),
        )
        .await;
        let id = created["id"].as_str().unwrap();
        let (st, archived) =
            call(&app, "POST", &format!("/v1/environments/{id}/archive"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(archived["state"], "archived");
        assert!(archived["archived_at"].is_string());
    }
}
