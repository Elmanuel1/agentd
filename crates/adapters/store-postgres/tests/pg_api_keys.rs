//! Workspace API keys over live Postgres: only the digest is stored, a key
//! resolves until it is revoked, and the digest is unique. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use ::api_keys::{ApiKey, ApiKeysService, ApiKeysStore};
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::ports::keys::{KeyPepper, KeySource};
use shared::ports::secrets::ExposeSecret;
use shared::{OrgId, Timestamp, WorkspaceId};
use store_postgres::schema::api_keys;
use store_postgres::{PgApiKeysStore, PgBackend};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn stamp() -> i64 {
    Timestamp::now().as_datetime().timestamp_micros()
}

#[tokio::test]
async fn a_key_resolves_until_it_is_revoked_and_only_its_digest_is_stored() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let pepper = Arc::new(KeyPepper::new(&[7; 32]));
    let keys = ApiKeysService::new(Arc::new(PgApiKeysStore::new(pg.clone())), pepper);
    let ws = format!("wrkspc_k{}", stamp());
    let (record, key) = keys.create("org_keys", &ws, "ci").await.unwrap();
    let ctx = keys.resolve(key.expose_secret()).await.unwrap().unwrap();
    assert_eq!((ctx.workspace.as_str(), ctx.api_key_id.as_str()), (ws.as_str(), record.id.as_str()));

    let mut conn = pg.diesel.get().await.unwrap();
    let (digest, hint): (Vec<u8>, String) = api_keys::table
        .filter(api_keys::api_key_id.eq(&record.id))
        .select((api_keys::key_digest, api_keys::partial_key_hint))
        .first(&mut conn)
        .await
        .unwrap();
    assert_eq!(digest.len(), 32);
    assert!(!hint.contains(&key.expose_secret()[20..90]));

    let listed = keys.list().await.unwrap();
    let mine = listed.iter().find(|k| k.id == record.id).unwrap();
    assert_eq!((mine.workspace.as_str(), mine.name.as_str()), (ws.as_str(), "ci"));
    assert!(keys.revoke(&record.id).await.unwrap());
    assert_eq!(keys.resolve(key.expose_secret()).await.unwrap(), None, "a revoke holds at once");
    assert!(!keys.revoke(&record.id).await.unwrap(), "revoking twice finds no active key");
}

#[tokio::test]
async fn a_digest_is_stored_once() {
    if !enabled() {
        return;
    }
    let store = PgApiKeysStore::new(backend().await);
    let key = |id: String| ApiKey {
        id,
        org: OrgId::new("org_keys"),
        workspace: WorkspaceId::new("wrkspc_keys"),
        name: "ci".into(),
        partial_key_hint: "sk-ant-api03-xxx...xxAA".into(),
        created_at: Timestamp::now(),
        revoked_at: None,
    };
    let digest = stamp().to_be_bytes().repeat(4);
    store.insert(&key(format!("apikey_a{}", stamp())), &digest).await.unwrap();
    assert!(store.insert(&key(format!("apikey_b{}", stamp())), &digest).await.is_err());
}

#[tokio::test]
async fn a_workspace_revoke_reaches_only_that_workspaces_active_keys() {
    if !enabled() {
        return;
    }
    let store = PgApiKeysStore::new(backend().await);
    let keys = ApiKeysService::new(Arc::new(store.clone()), Arc::new(KeyPepper::new(&[7; 32])));
    let (archived, kept) = (format!("wrkspc_a{}", stamp()), format!("wrkspc_k{}", stamp()));
    let (_, a1) = keys.create("org_keys", &archived, "a1").await.unwrap();
    let (_, a2) = keys.create("org_keys", &archived, "a2").await.unwrap();
    let (_, other_org) = keys.create("org_other", &archived, "o").await.unwrap();
    let (_, k) = keys.create("org_keys", &kept, "k").await.unwrap();
    let revoked =
        store.revoke_workspace(&OrgId::new("org_keys"), &WorkspaceId::new(&archived), Timestamp::now()).await;
    assert_eq!(revoked.unwrap(), 2);
    for gone in [&a1, &a2] {
        assert_eq!(keys.resolve(gone.expose_secret()).await.unwrap(), None);
    }
    for live in [&other_org, &k] {
        assert!(keys.resolve(live.expose_secret()).await.unwrap().is_some());
    }
    let again =
        store.revoke_workspace(&OrgId::new("org_keys"), &WorkspaceId::new(&archived), Timestamp::now()).await;
    assert_eq!(again.unwrap(), 0, "a revoked key is not revoked again");
}
