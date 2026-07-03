//! Axum routes for `/v1/agents` — thin: parse → service → respond.
//!
//! The router is mounted by `agentd-api`, which injects [`AuthContext`] and
//! the request-id/error-envelope middleware. Handlers here return
//! `Result<Json, ApiError>`; the envelope wrapping happens in one place.

use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, RequestId};
use axum::extract::{Extension, Path};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::AgentsService;
use crate::types::{CreateAgentRequest, UpdateAgentRequest};

/// Wrapper so `ApiError` can be returned from handlers directly.
#[derive(Debug)]
pub struct RouteError(pub ApiError);

impl From<ApiError> for RouteError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl IntoResponse for RouteError {
    fn into_response(self) -> Response {
        // Each response carries a fresh req_ id, like the live API.
        let rid = RequestId::generate();
        let body = self.0.to_envelope(&rid);
        let status = axum::http::StatusCode::from_u16(self.0.status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(body)).into_response()
    }
}

/// Build the `/v1/agents` router. State is injected via Extensions so the
/// composing crate (`agentd-api`) owns middleware ordering.
pub fn router() -> Router {
    Router::new()
        .route("/v1/agents", post(create).get(list))
        .route("/v1/agents/{id}", get(get_one).post(update))
        .route("/v1/agents/{id}/archive", post(archive))
        .route("/v1/agents/{id}/versions", get(versions))
}

async fn create(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateAgentRequest>,
) -> Result<Response, RouteError> {
    let agent = svc.create(&ctx, body).await?;
    Ok(Json(agent).into_response())
}

async fn list(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Response, RouteError> {
    let agents = svc.list(&ctx).await?;
    Ok(Json(Page::of(agents)).into_response())
}

async fn get_one(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let agent = svc.get(&ctx, &id).await?;
    Ok(Json(agent).into_response())
}

async fn update(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<UpdateAgentRequest>,
) -> Result<Response, RouteError> {
    let agent = svc.update(&ctx, &id, body).await?;
    Ok(Json(agent).into_response())
}

async fn archive(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let agent = svc.archive(&ctx, &id).await?;
    Ok(Json(agent).into_response())
}

async fn versions(
    Extension(svc): Extension<Arc<AgentsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    let list = svc.versions(&ctx, &id).await?;
    Ok(Json(Page::of(list)).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryAgentsStore;
    use agentd_core::limits::ModelCatalog;
    use agentd_core::{OrgId, WorkspaceId};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let svc = Arc::new(AgentsService::new(
            Arc::new(InMemoryAgentsStore::new()),
            ModelCatalog::default_catalog(),
        ));
        let ctx = AuthContext {
            org: OrgId("org_t".into()),
            workspace: WorkspaceId("wrkspc_01t".into()),
            api_key_id: "apikey_01t".into(),
        };
        router()
            .layer(Extension(svc))
            .layer(Extension(ctx))
    }

    async fn call(app: &Router, method: &str, uri: &str, body: Option<&str>) -> (StatusCode, serde_json::Value) {
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
        let v: serde_json::Value =
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    #[tokio::test]
    async fn create_then_get_roundtrip() {
        let app = app();
        let (st, created) = call(
            &app,
            "POST",
            "/v1/agents",
            Some(r#"{"name":"compat-probe-agent","model":"claude-sonnet-4-6","system":"probe","tools":[{"type":"agent_toolset_20260401"}]}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK); // live API returns 200 on create
        assert_eq!(created["type"], "agent");
        assert_eq!(created["version"], 1);
        let id = created["id"].as_str().unwrap();
        let (st, fetched) = call(&app, "GET", &format!("/v1/agents/{id}"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(fetched, created);
    }

    #[tokio::test]
    async fn list_empty_matches_live_shape() {
        let app = app();
        let (st, v) = call(&app, "GET", "/v1/agents", None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v, serde_json::json!({"data": []}));
    }

    #[tokio::test]
    async fn missing_model_gets_wire_envelope() {
        let app = app();
        let (st, v) = call(&app, "POST", "/v1/agents", Some(r#"{"name":"x"}"#)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "invalid_request_error");
        assert_eq!(v["error"]["message"], "model: Field required");
        assert!(v["request_id"].as_str().unwrap().starts_with("req_01"));
    }

    #[tokio::test]
    async fn malformed_id_is_400_with_live_message() {
        let app = app();
        let (st, v) = call(&app, "GET", "/v1/agents/agent_00bogus", None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"]["message"], "Invalid agent ID.");
    }

    #[tokio::test]
    async fn update_and_versions_flow() {
        let app = app();
        let (_, created) = call(
            &app,
            "POST",
            "/v1/agents",
            Some(r#"{"name":"a","model":"claude-sonnet-4-6"}"#),
        )
        .await;
        let id = created["id"].as_str().unwrap();
        let (st, updated) = call(
            &app,
            "POST",
            &format!("/v1/agents/{id}"),
            Some(r#"{"system":"v2 prompt"}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(updated["version"], 2);
        let (st, versions) = call(&app, "GET", &format!("/v1/agents/{id}/versions"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(versions["data"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn archive_route() {
        let app = app();
        let (_, created) = call(
            &app,
            "POST",
            "/v1/agents",
            Some(r#"{"name":"a","model":"claude-sonnet-4-6"}"#),
        )
        .await;
        let id = created["id"].as_str().unwrap();
        let (st, archived) = call(&app, "POST", &format!("/v1/agents/{id}/archive"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert!(archived["archived_at"].is_string());
    }
}
