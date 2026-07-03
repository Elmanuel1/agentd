//! Axum routes for `/v1/vaults` and nested `/credentials`.

use std::sync::Arc;

use agentd_core::{ApiError, AuthContext, Page, RequestId};
use axum::extract::{Extension, Path};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::VaultsService;
use crate::types::{
    CreateCredentialRequest, CreateVaultRequest, UpdateCredentialRequest, UpdateVaultRequest,
};

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

/// Build the vaults router.
pub fn router() -> Router {
    Router::new()
        .route("/v1/vaults", post(create_vault).get(list_vaults))
        .route(
            "/v1/vaults/{id}",
            get(get_vault).post(update_vault).delete(delete_vault),
        )
        .route("/v1/vaults/{id}/archive", post(archive_vault))
        .route(
            "/v1/vaults/{id}/credentials",
            post(create_credential).get(list_credentials),
        )
        .route(
            "/v1/vaults/{id}/credentials/{cid}",
            get(get_credential).post(update_credential).delete(delete_credential),
        )
        .route(
            "/v1/vaults/{id}/credentials/{cid}/archive",
            post(archive_credential),
        )
}

async fn create_vault(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Json(body): Json<CreateVaultRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_vault(&ctx, body).await?).into_response())
}

async fn list_vaults(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Response, RouteError> {
    Ok(Json(Page::of(svc.list_vaults(&ctx).await?)).into_response())
}

async fn get_vault(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_vault(&ctx, &id).await?).into_response())
}

async fn update_vault(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<UpdateVaultRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update_vault(&ctx, &id, body).await?).into_response())
}

async fn archive_vault(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive_vault(&ctx, &id).await?).into_response())
}

async fn delete_vault(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete_vault(&ctx, &id).await?).into_response())
}

async fn create_credential(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<CreateCredentialRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_credential(&ctx, &id, body).await?).into_response())
}

async fn list_credentials(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Result<Response, RouteError> {
    Ok(Json(Page::of(svc.list_credentials(&ctx, &id).await?)).into_response())
}

async fn get_credential(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, cid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.get_credential(&ctx, &id, &cid).await?).into_response())
}

async fn update_credential(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, cid)): Path<(String, String)>,
    Json(body): Json<UpdateCredentialRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.update_credential(&ctx, &id, &cid, body).await?).into_response())
}

async fn archive_credential(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, cid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.archive_credential(&ctx, &id, &cid).await?).into_response())
}

async fn delete_credential(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, cid)): Path<(String, String)>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.delete_credential(&ctx, &id, &cid).await?).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryVaultsStore;
    use agentd_core::{OrgId, WorkspaceId};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let svc = Arc::new(VaultsService::new(Arc::new(InMemoryVaultsStore::new())));
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
    async fn vault_and_credential_flow_no_secret_on_wire() {
        let app = app();
        let (st, vault) = call(
            &app,
            "POST",
            "/v1/vaults",
            Some(r#"{"display_name":"Alice"}"#),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let vid = vault["id"].as_str().unwrap();

        let (st, cred) = call(
            &app,
            "POST",
            &format!("/v1/vaults/{vid}/credentials"),
            Some(
                r#"{"display_name":"Slack","auth":{"type":"mcp_oauth","mcp_server_url":"https://mcp.slack.com/mcp","access_token":"xoxp-topsecret"}}"#,
            ),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(!cred.to_string().contains("xoxp-topsecret"));
        assert_eq!(cred["auth"]["type"], "mcp_oauth");

        // Duplicate key over HTTP → 409 with the wire envelope.
        let (st, err) = call(
            &app,
            "POST",
            &format!("/v1/vaults/{vid}/credentials"),
            Some(
                r#"{"display_name":"Slack2","auth":{"type":"mcp_oauth","mcp_server_url":"https://mcp.slack.com/mcp","access_token":"other"}}"#,
            ),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT);
        assert_eq!(err["error"]["type"], "invalid_request_error");
    }
}
