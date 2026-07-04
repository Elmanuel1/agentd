//! HTTP route coverage for /v1/files — multipart upload through download.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use agentd_core::{AuthContext, OrgId, WorkspaceId};
use agentd_files::store::InMemoryFilesStore;
use agentd_files::{routes, FilesService};
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app() -> Router {
    let svc = Arc::new(FilesService::new(Arc::new(InMemoryFilesStore::new())));
    let ctx = AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    };
    routes::router()
        .layer(Extension(svc))
        .layer(Extension(ctx))
}

const BOUNDARY: &str = "X-AGENTD-TEST-BOUNDARY";

fn multipart_body(filename: &str, content: &str, scope_id: Option<&str>) -> (String, String) {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: text/csv\r\n\r\n{content}\r\n"
    );
    if let Some(s) = scope_id {
        body.push_str(&format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"scope_id\"\r\n\r\n{s}\r\n"
        ));
    }
    body.push_str(&format!("--{BOUNDARY}--\r\n"));
    (
        format!("multipart/form-data; boundary={BOUNDARY}"),
        body,
    )
}

async fn upload(app: &Router, filename: &str, content: &str, scope: Option<&str>) -> serde_json::Value {
    let (ct, body) = multipart_body(filename, content, scope);
    let req = Request::builder()
        .method("POST")
        .uri("/v1/files")
        .header("content-type", ct)
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

#[tokio::test]
async fn multipart_upload_metadata_download_delete_flow() {
    let app = app();
    let meta = upload(&app, "data.csv", "a,b\n1,2\n", None).await;
    assert!(meta["id"].as_str().unwrap().starts_with("file_01"));
    assert_eq!(meta["filename"], "data.csv");
    assert_eq!(meta["mime_type"], "text/csv");
    assert_eq!(meta["size_bytes"], 8);
    assert_eq!(meta["type"], "file");
    let id = meta["id"].as_str().unwrap();

    // metadata
    let (st, body) = get(&app, &format!("/v1/files/{id}")).await;
    assert_eq!(st, StatusCode::OK);
    let got: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(got, meta);

    // download — raw bytes + content type
    let (st, bytes) = get(&app, &format!("/v1/files/{id}/content")).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(&bytes[..], b"a,b\n1,2\n");

    // delete then 404 envelope
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/v1/files/{id}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (st, body) = get(&app, &format!("/v1/files/{id}")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let err: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err["error"]["type"], "not_found_error");
}

#[tokio::test]
async fn list_with_scope_filter_over_http() {
    let app = app();
    upload(&app, "a.txt", "x", Some("sesn_01A")).await;
    upload(&app, "b.txt", "y", None).await;
    let (st, body) = get(&app, "/v1/files?scope_id=sesn_01A").await;
    assert_eq!(st, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["data"].as_array().unwrap().len(), 1);
    assert_eq!(v["data"][0]["filename"], "a.txt");
    let (_, body) = get(&app, "/v1/files").await;
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["data"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn upload_without_file_field_is_required_error() {
    let app = app();
    let body = format!("--{BOUNDARY}--\r\n");
    let req = Request::builder()
        .method("POST")
        .uri("/v1/files")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"]["message"], "file: Field required");
}

#[tokio::test]
async fn malformed_file_id_is_400() {
    let app = app();
    let (st, body) = get(&app, "/v1/files/file_00bogus").await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"]["message"], "Invalid file ID.");
}
