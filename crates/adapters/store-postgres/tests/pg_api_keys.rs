//! `PgApiKeys` against a live, Flyway-migrated Postgres. Gated by
//! `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use diesel::QueryDsl;
use diesel_async::RunQueryDsl;
use shared::ports::keys::{KeyPepper, KeySource};
use shared::ports::secrets::ExposeSecret;
use store_postgres::schema::api_keys;
use store_postgres::{PgApiKeys, PgBackend};

async fn backend() -> Option<PgBackend> {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return None;
    }
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL with AGENTD_PG_TESTS=1");
    Some(PgBackend::connect(&url).await.unwrap())
}

fn keys(backend: &PgBackend, pepper: u8) -> PgApiKeys {
    PgApiKeys::new(backend.clone(), Arc::new(KeyPepper::new(&[pepper; 32])))
}

fn workspace() -> String {
    format!("wrkspc_{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test]
async fn a_created_key_resolves_until_it_is_revoked() {
    let Some(backend) = backend().await else { return };
    let keys = keys(&backend, 7);
    let ws = workspace();
    let (record, key) = keys.create("org_keys", &ws, "ci").await.unwrap();
    assert!(record.id.starts_with("apikey_01") && record.revoked_at.is_none());
    assert!(key.expose_secret().starts_with("sk-ant-api03-"));
    assert_eq!(record.partial_key_hint.len(), 23);
    let ctx = keys.resolve(key.expose_secret()).await.unwrap().unwrap();
    assert_eq!(
        (ctx.org.as_str(), ctx.workspace.as_str(), ctx.api_key_id.as_str()),
        ("org_keys", &*ws, &*record.id)
    );
    assert!(keys.list().await.unwrap().iter().any(|r| r == &record));
    assert!(keys.revoke(&record.id).await.unwrap());
    assert_eq!(
        keys.resolve(key.expose_secret()).await.unwrap(),
        None,
        "a revoke holds from the next request"
    );
    assert!(!keys.revoke(&record.id).await.unwrap(), "revoking twice finds no active key");
    let listed = keys.list().await.unwrap().into_iter().find(|r| r.id == record.id).unwrap();
    assert!(listed.revoked_at.is_some());
}

#[tokio::test]
async fn only_the_exact_key_under_the_same_pepper_resolves() {
    let Some(backend) = backend().await else { return };
    let (_, key) = keys(&backend, 8).create("org_keys", &workspace(), "ci").await.unwrap();
    let key = key.expose_secret();
    assert_eq!(keys(&backend, 9).resolve(key).await.unwrap(), None, "another pepper verifies nothing");
    let mut altered = key.to_owned();
    altered.replace_range(20..21, if &key[20..21] == "A" { "B" } else { "A" });
    for wrong in [altered.as_str(), "", "sk-ant-api03-x", &key[..key.len() - 1]] {
        assert_eq!(keys(&backend, 8).resolve(wrong).await.unwrap(), None, "{wrong}");
    }
}

#[tokio::test]
async fn no_key_is_stored_only_its_digest() {
    let Some(backend) = backend().await else { return };
    let (record, key) = keys(&backend, 10).create("org_keys", &workspace(), "ci").await.unwrap();
    let mut conn = backend.diesel.get().await.unwrap();
    let row: (String, String, String, String, Vec<u8>, String) = api_keys::table
        .find(&record.id)
        .select((
            api_keys::api_key_id,
            api_keys::org_id,
            api_keys::workspace_id,
            api_keys::name,
            api_keys::key_digest,
            api_keys::partial_key_hint,
        ))
        .first(&mut conn)
        .await
        .unwrap();
    let key = key.expose_secret();
    let stored = format!("{row:?}{}", String::from_utf8_lossy(&row.4));
    assert!(!stored.contains(&key[16..key.len() - 4]), "the key's secret part is not in the row");
    assert_eq!(row.4.len(), 32);
}

#[tokio::test]
async fn ids_outside_the_charset_and_empty_names_are_refused() {
    let Some(backend) = backend().await else { return };
    let keys = keys(&backend, 11);
    for (org, ws, name) in [
        ("org/x", "wrkspc_a", "n"),
        ("org_a", "wrkspc a", "n"),
        ("org_a", "wrkspc_a", ""),
        ("org_a", "wrkspc_a", "a\tb"),
        ("org_a", "wrkspc_a", "a\nb"),
    ] {
        assert!(keys.create(org, ws, name).await.is_err(), "{org} {ws} {name:?}");
    }
}
