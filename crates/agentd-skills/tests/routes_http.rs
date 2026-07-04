//! HTTP route coverage for /v1/skills.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_core::{AuthContext, OrgId, WorkspaceId};
use agentd_skills::{routes, InMemorySkillsStore, SkillsService};
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> Router {
    let svc = Arc::new(SkillsService::new(Arc::new(InMemorySkillsStore::new())));
    let ctx = AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    };
    routes::router()
        .layer(Extension(svc))
        .layer(Extension(ctx))
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
            Some(s) => Body::from(s.to_owned()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn full_skill_lifecycle_over_http() {
    let app = app();
    // create
    let (st, skill) = call(
        &app,
        "POST",
        "/v1/skills",
        Some(r#"{"display_name":"citing-sources","description":"cite everything"}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(skill["type"], "skill");
    assert_eq!(skill["latest_version"], serde_json::Value::Null);
    let id = skill["id"].as_str().unwrap();

    // list + get
    let (st, listed) = call(&app, "GET", "/v1/skills", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);
    let (st, got) = call(&app, "GET", &format!("/v1/skills/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got, skill);

    // versions
    let (st, v1) = call(
        &app,
        "POST",
        &format!("/v1/skills/{id}/versions"),
        Some(r##"{"content":"# SKILL v1"}"##),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v1["version"], 1);
    assert_eq!(v1["type"], "skill_version");
    let (_, v2) = call(
        &app,
        "POST",
        &format!("/v1/skills/{id}/versions"),
        Some(r##"{"content":"# SKILL v2"}"##),
    )
    .await;
    assert_eq!(v2["version"], 2);

    let (st, versions) = call(&app, "GET", &format!("/v1/skills/{id}/versions"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(versions["data"].as_array().unwrap().len(), 2);

    let (st, got_v1) = call(&app, "GET", &format!("/v1/skills/{id}/versions/1"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got_v1["content"], "# SKILL v1");

    // delete a version, then the skill
    let (st, _) = call(&app, "DELETE", &format!("/v1/skills/{id}/versions/1"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&app, "GET", &format!("/v1/skills/{id}/versions/1"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(&app, "DELETE", &format!("/v1/skills/{id}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, err) = call(&app, "GET", &format!("/v1/skills/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(err["error"]["type"], "not_found_error");
}

#[tokio::test]
async fn required_and_invalid_id_errors_over_http() {
    let app = app();
    let (st, v) = call(&app, "POST", "/v1/skills", Some("{}")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "display_name: Field required");
    let (st, v) = call(&app, "GET", "/v1/skills/skill_00bogus", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["message"], "Invalid skill ID.");
    // version create on missing skill
    let ghost = agentd_skills::SkillId::generate();
    let (st, _) = call(
        &app,
        "POST",
        &format!("/v1/skills/{ghost}/versions"),
        Some(r#"{"content":"x"}"#),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
