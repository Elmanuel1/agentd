//! Axum routes for `/v1/organizations`.

use axum::extract::Extension;
use axum::routing::get;
use axum::{Json, Router};
use shared::AdminContext;

use crate::types::Organization;

/// Build the `/v1/organizations` router.
pub fn router() -> Router {
    Router::new().route("/v1/organizations/me", get(me))
}

async fn me(Extension(ctx): Extension<AdminContext>) -> Json<Organization> {
    Json(Organization::of(&ctx))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use shared::OrgId;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn me_names_the_admin_keys_organization() {
        let ctx = AdminContext { org: OrgId::new("org_a"), admin_key_id: "adminkey_a".into() };
        let app = router().layer(Extension(ctx));
        let req = Request::get("/v1/organizations/me").body(Body::empty()).unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body, serde_json::json!({"id": "org_a", "type": "organization", "name": "org_a"}));
    }
}
