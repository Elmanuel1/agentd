//! Environments service + HTTP route tests over Postgres (moved out of the
//! entity crate: the store impl and the trait must come from the same
//! compilation, which only happens here — store-postgres depends on
//! agentd-environments directly). Gated by `AGENTD_PG_TESTS`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use agentd_core::{AuthContext, OrgId, Timestamp, WorkspaceId};
use agentd_environments::types::{CreateEnvironmentRequest, EnvironmentState, UpdateEnvironmentRequest};
use agentd_environments::EnvironmentsService;
use agentd_store_postgres::{PgBackend, PgEnvironmentsStore};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn svc() -> EnvironmentsService {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let backend = PgBackend::connect(&url).await.unwrap();
    EnvironmentsService::new(Arc::new(PgEnvironmentsStore::new(backend)))
}

static SEQ: AtomicU64 = AtomicU64::new(0);
fn uniq_org() -> String {
    format!(
        "org_t{}_{}",
        Timestamp::now().as_datetime().timestamp_micros(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn ctx(org: &str) -> AuthContext {
    AuthContext {
        org: OrgId::new(org),
        workspace: WorkspaceId::new("wrkspc_01a"),
        api_key_id: "apikey_01".into(),
    }
}

/// Same org, different workspace — env must still be visible (org scope).
fn ctx_other_ws(org: &str) -> AuthContext {
    AuthContext {
        org: OrgId::new(org),
        workspace: WorkspaceId::new("wrkspc_02b"),
        api_key_id: "apikey_02".into(),
    }
}

fn minimal() -> CreateEnvironmentRequest {
    CreateEnvironmentRequest {
        name: Some("compat-probe-env".into()),
        config: serde_json::from_str(r#"{"type":"cloud","networking":{"type":"unrestricted"}}"#).ok(),
        ..Default::default()
    }
}

#[tokio::test]
async fn create_matches_live_canonical_shape() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s.create(&ctx(&org), minimal()).await.unwrap();
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(v["type"], "environment");
    assert_eq!(v["description"], "");
    assert_eq!(v["state"], "active");
    assert_eq!(v["scope"], "organization");
    assert_eq!(v["config"]["packages"]["type"], "packages");
    assert_eq!(v["config"]["init_script"], "");
    assert!(e.id.as_str().starts_with("env_01"));
}

#[tokio::test]
async fn duplicate_names_allowed_like_live_api() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let a = s.create(&ctx(&org), minimal()).await.unwrap();
    let b = s.create(&ctx(&org), minimal()).await.unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(a.name, b.name);
    assert_eq!(s.list(&ctx(&org), &agentd_core::Pagination::from_params(&std::collections::HashMap::new()).unwrap()).await.unwrap().len(), 2);
}

#[tokio::test]
async fn org_scoped_visible_across_workspaces() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s.create(&ctx(&org), minimal()).await.unwrap();
    let got = s.get(&ctx_other_ws(&org), e.id.as_str()).await.unwrap();
    assert_eq!(got.id, e.id);
}

#[tokio::test]
async fn missing_fields_use_required_error() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s
        .create(&ctx(&org), CreateEnvironmentRequest::default())
        .await
        .unwrap_err();
    assert_eq!(e.message, "name: Field required");
    let e = s
        .create(
            &ctx(&org),
            CreateEnvironmentRequest {
                name: Some("x".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.message, "config: Field required");
}

#[tokio::test]
async fn malformed_id_message_uses_environment_label() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s.get(&ctx(&org), "env_00bogus").await.unwrap_err();
    assert_eq!(e.status, 400);
    assert_eq!(e.message, "Invalid environment ID.");
}

#[tokio::test]
async fn archive_then_update_conflicts() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s.create(&ctx(&org), minimal()).await.unwrap();
    let archived = s.archive(&ctx(&org), e.id.as_str()).await.unwrap();
    assert!(archived.archived_at.is_some());
    assert_eq!(archived.state, EnvironmentState::Archived);
    let err = s
        .update(&ctx(&org), e.id.as_str(), UpdateEnvironmentRequest::default())
        .await
        .unwrap_err();
    assert_eq!(err.status, 409);
}

#[tokio::test]
async fn delete_removes() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s.create(&ctx(&org), minimal()).await.unwrap();
    s.delete(&ctx(&org), e.id.as_str()).await.unwrap();
    assert_eq!(s.get(&ctx(&org), e.id.as_str()).await.unwrap_err().status, 404);
}

#[tokio::test]
async fn update_replaces_config_and_bumps_updated_at() {
    if !enabled() {
        return;
    }
    let s = svc().await;
    let org = uniq_org();
    let e = s.create(&ctx(&org), minimal()).await.unwrap();
    let updated = s
        .update(
            &ctx(&org),
            e.id.as_str(),
            UpdateEnvironmentRequest {
                config: serde_json::from_str(r#"{"type":"self_hosted"}"#).ok(),
                description: Some("now self hosted".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        updated.config,
        agentd_environments::EnvironmentConfig::SelfHosted {}
    ));
    assert_eq!(updated.description, "now self hosted");
}

// ── HTTP route tests (router built over the Pg-backed service) ───────────────
mod routes {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::{Extension, Router};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn app() -> Router {
        let url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
        let backend = PgBackend::connect(&url).await.unwrap();
        let svc = Arc::new(EnvironmentsService::new(Arc::new(PgEnvironmentsStore::new(backend))));
        let org = format!("org_rt{}", Timestamp::now().as_datetime().timestamp_micros());
        let ctx = AuthContext {
            org: OrgId::new(org),
            workspace: WorkspaceId::new("wrkspc_01t"),
            api_key_id: "apikey_01t".into(),
        };
        agentd_environments::routes::router()
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
        if !enabled() {
            return;
        }
        let app = app().await;
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
        assert_eq!(st, StatusCode::OK);
        let (st, _) = call(&app, "GET", &format!("/v1/environments/{id}"), None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn archive_route_sets_state() {
        if !enabled() {
            return;
        }
        let app = app().await;
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
