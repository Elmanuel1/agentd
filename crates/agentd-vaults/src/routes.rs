//! Axum routes for `/v1/vaults` and nested `/credentials`.

use std::collections::HashMap;
use std::sync::Arc;

use agentd_core::{
    ApiJson, AuthContext, Page, Pagination, RouteError,
};
use axum::extract::{Extension, Path, Query};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use crate::service::VaultsService;
use crate::types::{
    CreateCredentialRequest, CreateVaultRequest, UpdateCredentialRequest, UpdateVaultRequest,
};

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
    ApiJson(body): ApiJson<CreateVaultRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_vault(&ctx, body).await?).into_response())
}

async fn list_vaults(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_list_params(&params)?;
    Ok(Json(Page::paginate(svc.list_vaults(&ctx).await?, &page)).into_response())
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
    ApiJson(body): ApiJson<UpdateVaultRequest>,
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
    ApiJson(body): ApiJson<CreateCredentialRequest>,
) -> Result<Response, RouteError> {
    Ok(Json(svc.create_credential(&ctx, &id, body).await?).into_response())
}

async fn list_credentials(
    Extension(svc): Extension<Arc<VaultsService>>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, RouteError> {
    let page = Pagination::from_list_params(&params)?;
    Ok(Json(Page::paginate(svc.list_credentials(&ctx, &id).await?, &page)).into_response())
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
    ApiJson(body): ApiJson<UpdateCredentialRequest>,
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
