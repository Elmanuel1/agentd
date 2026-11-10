//! The auth edge picks the key store by route: `/v1/organizations` routes take
//! admin keys only, every other route workspace keys only. Hermetic, over
//! stand-in routes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use api::edge;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use http_body_util::BodyExt;
use shared::{AdminContext, AuthContext};
use test_support::{StaticAdminKeys, StaticKeySource};
use tower::ServiceExt;

use api::edge::reach::{Grants, SessionGrants};
use api::edge::scoped::WorkAccess;
use shared::{ApiError, OrgId, Pagination, Timestamp, WorkspaceId};
use work::store::{Counts, Reach, Scope, StoreError};
use work::{WorkItem, WorkReads};

/// No work items: these tests send no work token or environment key.
struct NoWork;

#[async_trait::async_trait]
impl WorkReads for NoWork {
    async fn get(&self, _: &WorkspaceId, _: &str, _: &Reach) -> Result<Option<WorkItem>, StoreError> {
        Ok(None)
    }
    async fn find_on_queue(&self, _: &str, _: &str) -> Result<Option<WorkItem>, StoreError> {
        Ok(None)
    }
    async fn list(&self, _: &Scope, _: &Pagination) -> Result<Vec<WorkItem>, StoreError> {
        Ok(vec![])
    }
    async fn counts(&self, _: &Scope, _: std::time::Duration, _: Timestamp) -> Result<Counts, StoreError> {
        Ok(Counts::default())
    }
    async fn runtime_owner(&self, _: &str) -> Result<Option<(OrgId, WorkspaceId)>, StoreError> {
        Ok(None)
    }
}

struct NoGrants;

#[async_trait::async_trait]
impl SessionGrants for NoGrants {
    async fn grants(&self, _: &AuthContext, _: &str) -> Result<Grants, ApiError> {
        Ok(Grants::default())
    }
}

const WORKSPACE_KEY: &str = "sk-ant-api03-edge";
const ADMIN_KEY: &str = "sk-ant-admin01-edge";

async fn org(Extension(ctx): Extension<AdminContext>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"org": ctx.org.as_str(), "admin_key_id": ctx.admin_key_id}))
}

async fn workspace(Extension(ctx): Extension<AuthContext>) -> Json<serde_json::Value> {
    Json(serde_json::json!({"workspace": ctx.workspace.as_str()}))
}

fn app() -> Router {
    let mut keys = StaticKeySource::default();
    keys.register(WORKSPACE_KEY, "org_e", "wrkspc_e", "apikey_e");
    let mut admin_keys = StaticAdminKeys::default();
    admin_keys.register(ADMIN_KEY, "org_e", "adminkey_e");
    let routes = Router::new().route("/v1/organizations/me", get(org)).route("/v1/things", get(workspace));
    edge::wrap(
        routes,
        edge::auth::Credentials {
            keys: Arc::new(keys),
            admin_keys: Arc::new(admin_keys),
            environment_keys: Arc::new(test_support::NoEnvironmentKeys),
            work: WorkAccess {
                tokens: work::WorkTokens::new(b"admin-edge".to_vec()),
                items: Arc::new(NoWork),
                grants: Arc::new(NoGrants),
            },
        },
        Arc::from(vec![]),
        Arc::new(api::telemetry::Metrics::global()),
    )
}

async fn get_with(path: &str, key: &str) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let req = Request::get(path)
        .header("anthropic-version", "2023-06-01")
        .header("x-api-key", key)
        .body(Body::empty())
        .unwrap();
    let resp = app().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn an_admin_key_reaches_organizations_routes_with_its_org() {
    let (status, headers, body) = get_with("/v1/organizations/me", ADMIN_KEY).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"org": "org_e", "admin_key_id": "adminkey_e"}));
    assert_eq!(headers["anthropic-organization-id"], "org_e");
    assert!(headers.get("anthropic-workspace-id").is_none());
}

#[tokio::test]
async fn a_workspace_key_is_refused_on_organizations_routes() {
    let (status, _, body) = get_with("/v1/organizations/me", WORKSPACE_KEY).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["type"], "authentication_error");
    assert_eq!(body["error"]["message"], "invalid x-api-key");
}

#[tokio::test]
async fn an_admin_key_is_refused_on_workspace_routes() {
    let (status, _, body) = get_with("/v1/things", ADMIN_KEY).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"]["message"], "invalid x-api-key");
    let (status, headers, body) = get_with("/v1/things", WORKSPACE_KEY).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["workspace"], "wrkspc_e");
    assert_eq!(headers["anthropic-workspace-id"], "wrkspc_e");
}
