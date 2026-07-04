//! HTTP route coverage for the memory endpoints not covered by the unit
//! route tests: store get/list/archive/delete, memory get/delete with
//! preconditions, version get/redact.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_core::{AuthContext, OrgId, WorkspaceId};
use agentd_memory::{routes, InMemoryMemoryStore, MemoriesService};
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> Router {
    let svc = Arc::new(MemoriesService::new(Arc::new(InMemoryMemoryStore::new())));
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
async fn store_lifecycle_over_http() {
    let app = app();
    let (st, store) = call(
        &app,
        "POST",
        "/v1/memory_stores",
        Some(r#"{"name":"s","description":"d"}"#),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let sid = store["id"].as_str().unwrap();

    let (st, listed) = call(&app, "GET", "/v1/memory_stores", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);

    let (st, got) = call(&app, "GET", &format!("/v1/memory_stores/{sid}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got, store);

    let (st, archived) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/archive"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(archived["archived_at"].is_string());

    let (st, _) = call(&app, "DELETE", &format!("/v1/memory_stores/{sid}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(&app, "GET", &format!("/v1/memory_stores/{sid}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn memory_delete_with_precondition_over_http() {
    let app = app();
    let (_, store) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
    let sid = store["id"].as_str().unwrap();
    let (_, mem) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/a.md","content":"one"}"#),
    )
    .await;
    let mid = mem["id"].as_str().unwrap();
    let sha = mem["content_sha256"].as_str().unwrap();

    // Wrong precondition → 409 typed.
    let wrong = "a".repeat(64);
    let (st, err) = call(
        &app,
        "DELETE",
        &format!("/v1/memory_stores/{sid}/memories/{mid}?expected_content_sha256={wrong}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(err["error"]["type"], "memory_precondition_failed_error");

    // Matching precondition succeeds.
    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/v1/memory_stores/{sid}/memories/{mid}?expected_content_sha256={sha}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/v1/memory_stores/{sid}/memories/{mid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_memories_with_prefix_and_basic_view_over_http() {
    let app = app();
    let (_, store) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
    let sid = store["id"].as_str().unwrap();
    for p in ["/notes/a.md", "/other/b.md"] {
        call(
            &app,
            "POST",
            &format!("/v1/memory_stores/{sid}/memories"),
            Some(&format!(r#"{{"path":"{p}","content":"c"}}"#)),
        )
        .await;
    }
    let (st, notes) = call(
        &app,
        "GET",
        &format!("/v1/memory_stores/{sid}/memories?path_prefix=/notes/"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(notes["data"].as_array().unwrap().len(), 1);
    // Basic list view: no content field.
    assert!(notes["data"][0].get("content").is_none());
    // Full view includes it.
    let (_, full) = call(
        &app,
        "GET",
        &format!("/v1/memory_stores/{sid}/memories?view=full"),
        None,
    )
    .await;
    assert_eq!(full["data"][0]["content"], "c");
}

#[tokio::test]
async fn version_get_and_redact_over_http() {
    let app = app();
    let (_, store) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
    let sid = store["id"].as_str().unwrap();
    let (_, mem) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/secret.md","content":"leaked-key"}"#),
    )
    .await;
    let vid = mem["memory_version_id"].as_str().unwrap();

    // Full version fetch includes content.
    let (st, version) = call(
        &app,
        "GET",
        &format!("/v1/memory_stores/{sid}/memory_versions/{vid}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(version["content"], "leaked-key");
    assert_eq!(version["operation"], "created");

    // Redact scrubs it.
    let (st, redacted) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memory_versions/{vid}/redact"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(redacted["content_sha256"], serde_json::Value::Null);
    assert_eq!(redacted["redacted_by"]["type"], "api_actor");
    let (_, after) = call(
        &app,
        "GET",
        &format!("/v1/memory_stores/{sid}/memory_versions/{vid}"),
        None,
    )
    .await;
    assert!(after.get("content").is_none() || after["content"].is_null());
}

#[tokio::test]
async fn update_with_precondition_over_http() {
    let app = app();
    let (_, store) = call(&app, "POST", "/v1/memory_stores", Some(r#"{"name":"m"}"#)).await;
    let sid = store["id"].as_str().unwrap();
    let (_, mem) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories"),
        Some(r#"{"path":"/a.md","content":"current"}"#),
    )
    .await;
    let mid = mem["id"].as_str().unwrap();
    let sha = mem["content_sha256"].as_str().unwrap();
    // Bad format → the exact live 400.
    let (st, err) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories/{mid}"),
        Some(r#"{"content":"x","precondition":{"type":"content_sha256","content_sha256":"deadbeef"}}"#),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        err["error"]["message"],
        "precondition.content_sha256: value length must be 64 characters (and 1 more validation errors)"
    );
    // Matching sha → 200 + new head.
    let (st, updated) = call(
        &app,
        "POST",
        &format!("/v1/memory_stores/{sid}/memories/{mid}"),
        Some(&format!(
            r#"{{"content":"next","precondition":{{"type":"content_sha256","content_sha256":"{sha}"}}}}"#
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_ne!(updated["memory_version_id"], mem["memory_version_id"]);
}
