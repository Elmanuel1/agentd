//! Integration tests for the V2 entity stores (environments, vaults,
//! memory, sessions) against a live Postgres + object store.
//!
//! Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`; blob-backed tests also need
//! `AGENTD_AWS_ENDPOINT` (an S3 emulator) and skip otherwise. Run:
//! ```sh
//! AGENTD_PG_TESTS=1 DATABASE_URL=postgres://agentd:agentd@localhost:5432/agentd \
//!   AGENTD_AWS_ENDPOINT=http://localhost:4566 AWS_ACCESS_KEY_ID=test \
//!   AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-east-1 \
//!   cargo test -p store-postgres --test pg_entities
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use secrets_blob::BlobSecretStore;
use serde_json::json;
use shared::{AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{
    PgBackend, PgEnvironmentsStore, PgMemoryStore, PgSessionEventLog, PgSessionsStore, PgVaultsStore,
};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

/// Build a per-test object store, or `None` if no endpoint is configured.
async fn s3(tag: &str) -> Option<Arc<dyn BlobStore>> {
    std::env::var("AGENTD_AWS_ENDPOINT").ok()?;
    let bucket = format!("agentdent{tag}{}", stamp());
    Some(Arc::new(S3BlobStore::connect(&s3_config(bucket)).await.unwrap()))
}

/// A test bucket on the emulator at `AGENTD_AWS_ENDPOINT`, created on connect.
fn s3_config(bucket: String) -> S3Config {
    S3Config {
        endpoint: std::env::var("AGENTD_AWS_ENDPOINT").ok(),
        path_style: true,
        create_bucket: true,
        ..S3Config::new(bucket)
    }
}

fn stamp() -> i64 {
    Timestamp::now().as_datetime().timestamp_micros()
}

fn uniq_ws(tag: &str) -> WorkspaceId {
    WorkspaceId::new(format!("wrkspc_{tag}{}", stamp()))
}

fn ctx_ws(ws: &WorkspaceId) -> AuthContext {
    AuthContext {
        org: OrgId::new(format!("org_{}", ws.as_str())),
        workspace: ws.clone(),
        api_key_id: "apikey_ent".into(),
    }
}

fn ctx_org(tag: &str) -> AuthContext {
    let org = format!("org_{tag}{}", stamp());
    AuthContext {
        org: OrgId::new(org),
        workspace: WorkspaceId::new("wrkspc_ent"),
        api_key_id: "apikey_ent".into(),
    }
}

#[tokio::test]
async fn pg_environments_service_roundtrip() {
    if !enabled() {
        eprintln!("skipping (set AGENTD_PG_TESTS=1)");
        return;
    }
    use environments::EnvironmentsService;
    let svc = EnvironmentsService::new(Arc::new(PgEnvironmentsStore::new(backend().await)));
    let ctx = ctx_org("env");
    let env = svc
        .create(
            &ctx,
            serde_json::from_value(json!({
                "name": "prod",
                "config": {"type": "cloud", "networking": {"type": "unrestricted"}}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(env.name, "prod");
    assert_eq!(svc.get(&ctx, env.id.as_str()).await.unwrap().id, env.id);
    assert_eq!(
        svc.list(&ctx, false, &shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap())
            .await
            .unwrap()
            .len(),
        1
    );
    // org isolation
    assert!(svc.get(&ctx_org("other"), env.id.as_str()).await.is_err());
    svc.delete(&ctx, env.id.as_str()).await.unwrap();
    assert!(svc.get(&ctx, env.id.as_str()).await.is_err());
}

#[tokio::test]
async fn pg_vaults_secret_sealed_to_object_store() {
    if !enabled() {
        return;
    }
    let Some(secrets) = s3("vaults").await else {
        eprintln!("skipping vaults (needs AGENTD_AWS_ENDPOINT)");
        return;
    };
    use vaults::store::VaultsStore;
    use vaults::types::CredentialAuthInput;
    use vaults::VaultsService;
    let store = Arc::new(PgVaultsStore::new(backend().await, Arc::new(BlobSecretStore::new(secrets))));
    let svc = VaultsService::new(store.clone());
    let ctx = ctx_ws(&uniq_ws("vaults"));
    let vault =
        svc.create_vault(&ctx, serde_json::from_value(json!({"display_name": "v"})).unwrap()).await.unwrap();
    let cred = svc
        .create_credential(
            &ctx,
            vault.id.as_str(),
            serde_json::from_value(json!({
                "display_name": "c",
                "auth": {"type": "environment_variable", "secret_name": "API_KEY",
                         "secret_value": "s3cr3t", "networking": {"type": "unrestricted"}}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    // Live conformance: credential carries vault_id + metadata, and the env-var
    // wire auth includes injection_location {body,header}.
    let wire = serde_json::to_value(&cred).unwrap();
    assert_eq!(wire["vault_id"], vault.id.as_str());
    assert_eq!(wire["auth"]["injection_location"], json!({"body": true, "header": true}));
    assert_eq!(wire["auth"]["networking"]["type"], "unrestricted");
    // The secret is never in the DB — it round-trips through the secret store.
    let stored = store.get_credential(&ctx.workspace, &vault.id, &cred.id).await.unwrap();
    match stored.auth {
        Some(CredentialAuthInput::EnvironmentVariable { secret_value, .. }) => {
            assert_eq!(secret_value.expose(), "s3cr3t");
        }
        other => panic!("expected sealed env-var secret, got {other:?}"),
    }
}

#[tokio::test]
async fn pg_memory_list_directory_browse_matches_live() {
    if !enabled() {
        return;
    }
    let Some(blobs) = s3("memlist").await else {
        return;
    };
    use memory::MemoriesService;
    let svc = MemoriesService::new(Arc::new(PgMemoryStore::new(backend().await, blobs)));
    let ctx = ctx_ws(&uniq_ws("memlist"));
    let ms = svc
        .create_store(&ctx, serde_json::from_value(json!({"name": "m", "description": ""})).unwrap())
        .await
        .unwrap();
    for p in ["/a.md", "/notes/b.md", "/notes/sub/c.md", "/rules/d.md"] {
        svc.create_memory(
            &ctx,
            ms.id.as_str(),
            serde_json::from_value(json!({"path": p, "content": "x"})).unwrap(),
        )
        .await
        .unwrap();
    }

    // depth=1, order_by=path → leaf /a.md + collapsed /notes/ and /rules/,
    // exactly as the live API returns (probed 2026-07-05).
    let page = svc
        .list_memories(
            &ctx,
            ms.id.as_str(),
            &memory::service::MemoryListQuery {
                path_prefix: Some("/"),
                order_by: Some("path"),
                depth: 1,
                view: memory::service::View::Basic,
            },
            &shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap(),
        )
        .await
        .unwrap();
    let v = serde_json::to_value(&page).unwrap();
    let shapes: Vec<(&str, &str)> = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["path"].as_str().unwrap(), d["type"].as_str().unwrap()))
        .collect();
    assert_eq!(shapes, vec![("/a.md", "memory"), ("/notes/", "memory_prefix"), ("/rules/", "memory_prefix")]);
    assert!(v["prefixes"].as_array().unwrap().is_empty(), "prefixes is []");
    assert!(v["data"][0].get("content").is_none(), "list omits content");

    // depth=2 expands one more level, collapsing /notes/sub/.
    let page2 = svc
        .list_memories(
            &ctx,
            ms.id.as_str(),
            &memory::service::MemoryListQuery {
                path_prefix: Some("/"),
                order_by: Some("path"),
                depth: 2,
                view: memory::service::View::Basic,
            },
            &shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap(),
        )
        .await
        .unwrap();
    let v2 = serde_json::to_value(&page2).unwrap();
    let shapes2: Vec<(&str, &str)> = v2["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["path"].as_str().unwrap(), d["type"].as_str().unwrap()))
        .collect();
    assert_eq!(
        shapes2,
        vec![
            ("/a.md", "memory"),
            ("/notes/b.md", "memory"),
            ("/notes/sub/", "memory_prefix"),
            ("/rules/d.md", "memory")
        ]
    );

    // invalid order_by → the exact live 400 message.
    let err = svc
        .list_memories(
            &ctx,
            ms.id.as_str(),
            &memory::service::MemoryListQuery {
                path_prefix: None,
                order_by: Some("bogus"),
                depth: 0,
                view: memory::service::View::Basic,
            },
            &shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.message, "order_by must be one of: created_at, path");

    // no depth → flat list of all memories (no prefixes).
    let flat = svc
        .list_memories(
            &ctx,
            ms.id.as_str(),
            &memory::service::MemoryListQuery {
                path_prefix: None,
                order_by: None,
                depth: 0,
                view: memory::service::View::Basic,
            },
            &shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(flat.data.len(), 4);
}

#[tokio::test]
async fn pg_memory_versioned_with_content_and_rollback() {
    if !enabled() {
        return;
    }
    let Some(blobs) = s3("memory").await else {
        eprintln!("skipping memory (needs AGENTD_AWS_ENDPOINT)");
        return;
    };
    use memory::MemoriesService;
    let svc = MemoriesService::new(Arc::new(PgMemoryStore::new(backend().await, blobs)));
    let ctx = ctx_ws(&uniq_ws("memory"));
    let ms = svc
        .create_store(&ctx, serde_json::from_value(json!({"name": "m", "description": ""})).unwrap())
        .await
        .unwrap();
    let m1 = svc
        .create_memory(
            &ctx,
            ms.id.as_str(),
            serde_json::from_value(json!({"path": "/notes.md", "content": "hello"})).unwrap(),
        )
        .await
        .unwrap();
    let v1 = m1.memory_version_id.clone();
    let m2 = svc
        .update_memory(
            &ctx,
            ms.id.as_str(),
            m1.id.as_str(),
            serde_json::from_value(json!({"content": "world"})).unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(m2.memory_version_id, v1, "update appends a new head version");
    // v1 content is fetched from object storage
    let gv1 = svc.get_version(&ctx, ms.id.as_str(), v1.as_str()).await.unwrap();
    assert_eq!(gv1.content.as_deref(), Some("hello"));
    // O(1) rollback moves the head back to v1
    let rolled = svc.rollback(&ctx, ms.id.as_str(), m1.id.as_str(), v1.as_str()).await.unwrap();
    assert_eq!(rolled.memory_version_id, v1);
}

#[tokio::test]
async fn pg_sessions_shared_event_log() {
    if !enabled() {
        return;
    }
    use sessions::events::{Event, EventPayload, UserEvent};
    use sessions::store::{SessionEventLog, SessionsStore};
    use sessions::types::Session;
    let store = PgSessionsStore::new(backend().await);
    let log = PgSessionEventLog::new(backend().await);
    let ws = uniq_ws("sessions");
    let session: Session =
        serde_json::from_str(include_str!("../../../../conformance/fixtures/session.create.json")).unwrap();

    store.insert(&ws, session.clone()).await.unwrap();
    assert_eq!(store.get(&ws, &session.id).await.unwrap().id, session.id);

    let e1 = Event::new(EventPayload::User(UserEvent::Interrupt {}));
    let e2 = Event::new(EventPayload::User(UserEvent::Interrupt {}));
    log.append_event(&ws, &session.id, e1.clone()).await.unwrap();
    log.append_event(&ws, &session.id, e2.clone()).await.unwrap();

    let events = log.events(&ws, &session.id).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].id, e1.id, "log returns append order (by seq)");

    // replace stamps processed_at in place — no new row.
    let mut e1p = e1.clone();
    e1p.processed_at = Some(Timestamp::now());
    log.replace_event(&ws, &session.id, e1p).await.unwrap();
    let events = log.events(&ws, &session.id).await.unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[0].processed_at.is_some());

    store.delete(&ws, &session.id).await.unwrap();
    assert!(store.get(&ws, &session.id).await.is_err());
}

#[tokio::test]
async fn pg_memory_overlapping_paths_written_at_once_admit_one() {
    if !enabled() {
        return;
    }
    let Some(blobs) = s3("memrace").await else {
        return;
    };
    use memory::MemoriesService;
    let svc = Arc::new(MemoriesService::new(Arc::new(PgMemoryStore::new(backend().await, blobs))));
    let ctx = ctx_ws(&uniq_ws("memrace"));
    let ms = svc.create_store(&ctx, serde_json::from_value(json!({"name": "m"})).unwrap()).await.unwrap();
    for round in 0..10 {
        let (dir, file) = (format!("/r{round}"), format!("/r{round}/a.md"));
        let writes = [dir, file].map(|path| {
            let (svc, ctx, id) = (svc.clone(), ctx.clone(), ms.id.to_string());
            tokio::spawn(async move {
                let req = serde_json::from_value(json!({"path": path, "content": "x"})).unwrap();
                svc.create_memory(&ctx, &id, req).await
            })
        });
        let mut ok = 0;
        for w in writes {
            ok += usize::from(w.await.unwrap().is_ok());
        }
        assert_eq!(ok, 1, "round {round}: exactly one of a path and its child is admitted");
    }
}

#[tokio::test]
async fn pg_memory_concurrent_updates_never_lose_a_version() {
    if !enabled() {
        return;
    }
    let Some(blobs) = s3("memupd").await else {
        return;
    };
    use memory::MemoriesService;
    let svc = Arc::new(MemoriesService::new(Arc::new(PgMemoryStore::new(backend().await, blobs))));
    let ctx = ctx_ws(&uniq_ws("memupd"));
    let ms = svc.create_store(&ctx, serde_json::from_value(json!({"name": "m"})).unwrap()).await.unwrap();
    let req = serde_json::from_value(json!({"path": "/c.md", "content": "0"})).unwrap();
    let m = svc.create_memory(&ctx, ms.id.as_str(), req).await.unwrap();
    let writes = (1..=8).map(|i| {
        let (svc, ctx, sid, mid) = (svc.clone(), ctx.clone(), ms.id.to_string(), m.id.to_string());
        tokio::spawn(async move {
            let req = serde_json::from_value(json!({"content": format!("{i}")})).unwrap();
            svc.update_memory(&ctx, &sid, &mid, req).await
        })
    });
    let mut applied = 0;
    for w in writes.collect::<Vec<_>>() {
        match w.await.unwrap() {
            Ok(_) => applied += 1,
            Err(e) => assert_eq!(e.status, 409, "a lost race is a 409: {e:?}"),
        }
    }
    assert!(applied >= 1);
    let filter = memory::store::VersionFilter { memory_id: Some(m.id.to_string()), ..Default::default() };
    let page = shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap();
    let view = memory::service::View::Full;
    let versions = svc.list_versions(&ctx, ms.id.as_str(), &filter, view, &page).await.unwrap();
    assert_eq!(versions.len(), applied + 1, "one version per applied update, plus the create");
    let head = svc.get_memory(&ctx, ms.id.as_str(), m.id.as_str(), view).await.unwrap();
    assert_eq!(head.memory_version_id, versions[0].id, "the head is the newest version");
    assert_eq!(head.content, versions[0].content);
}
