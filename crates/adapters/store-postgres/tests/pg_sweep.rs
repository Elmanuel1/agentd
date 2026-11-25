//! The cleanup sweep removes the secrets and blobs no row names once they are a day old, and
//! leaves the rest. Gated by `AGENTD_PG_TESTS=1`, `DATABASE_URL` and `AGENTD_AWS_ENDPOINT` (S3).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use shared::Timestamp;
use shared::hash::SecretKeys;
use shared::ports::secrets::{SecretKey, SecretStore, SecretString, Stored};
use store_postgres::{PgBackend, PgSecretStore, Sweep};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1") && std::env::var("AGENTD_AWS_ENDPOINT").is_ok()
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn stamp() -> i64 {
    Timestamp::now().as_datetime().timestamp_micros()
}

fn keys() -> SecretKeys {
    SecretKeys::parse("k1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f").unwrap()
}

async fn exec(pg: &PgBackend, sql: &str, ws: &str) {
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query(sql).bind::<Text, _>(ws).execute(&mut *conn).await.unwrap();
}

#[tokio::test]
async fn a_day_old_secret_or_blob_no_row_names_is_removed_and_the_rest_stays() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let ws = format!("wrkspc_sweep{}", stamp());
    exec(&pg, "INSERT INTO credentials (workspace_id, vault_id, credential_id, display_name, has_secret, created_at, doc)
               VALUES ($1, 'vlt_a', 'vcrd_named', 'c', true, now(), '{}')", &ws).await;
    exec(
        &pg,
        "INSERT INTO files (workspace_id, file_id, filename, size_bytes, created_at, doc)
               VALUES ($1, 'file_named', 'f', 1, now(), '{}')",
        &ws,
    )
    .await;
    exec(
        &pg,
        "INSERT INTO skill_versions (workspace_id, skill_id, version, created_at, doc)
               VALUES ($1, 'skill_a', 1, now(), '{\"id\": \"skver_named\"}')",
        &ws,
    )
    .await;
    exec(&pg, "INSERT INTO memory_versions (workspace_id, store_id, memory_id, version_id, has_blob, created_at, doc)
               VALUES ($1, 'memstore_a', 'mem_a', 'memver_named', true, now(), '{}')", &ws).await;

    let secrets = Arc::new(PgSecretStore::new(pg.clone(), keys()));
    let key = |c: &str| SecretKey::new(&ws, "vlt_a", c).unwrap();
    secrets.put(&key("vcrd_named"), &SecretString::from("s")).await.unwrap();
    secrets.put(&key("vcrd_orphan"), &SecretString::from("s")).await.unwrap();
    let blobs: Arc<dyn BlobStore> = Arc::new(
        S3BlobStore::connect(&S3Config {
            endpoint: std::env::var("AGENTD_AWS_ENDPOINT").ok(),
            path_style: true,
            create_bucket: true,
            ..S3Config::new(format!("agentdsweep{}", stamp()))
        })
        .await
        .unwrap(),
    );
    let named = [
        format!("{ws}/file_named"),
        format!("skills/{ws}/skill_a/skver_named"),
        format!("memory/{ws}/memstore_a/memver_named"),
    ];
    let orphans = [
        format!("{ws}/file_orphan"),
        format!("skills/{ws}/skill_a/skver_orphan"),
        format!("memory/{ws}/memstore_a/memver_orphan"),
    ];
    for key in named.iter().chain(&orphans) {
        blobs.put(key, bytes::Bytes::from_static(b"b")).await.unwrap();
    }
    let sweep = Sweep::new(pg.clone(), secrets.clone(), blobs.clone());

    assert!(sweep.run(Timestamp::now()).await.unwrap());
    for key in named.iter().chain(&orphans) {
        assert!(blobs.get(key).await.is_ok(), "{key} is under a day old and stays");
    }
    assert!(secrets.get(&key("vcrd_orphan")).await.is_ok(), "a secret under a day old stays");

    let later = Timestamp::from_datetime(Timestamp::now().as_datetime() + chrono::Duration::days(2));
    assert!(sweep.run(later).await.unwrap());
    assert!(secrets.get(&key("vcrd_named")).await.is_ok());
    assert!(secrets.get(&key("vcrd_orphan")).await.is_err(), "no credential names it");
    for key in &named {
        assert!(blobs.get(key).await.is_ok(), "{key} has its row");
    }
    for key in &orphans {
        assert!(blobs.get(key).await.is_err(), "{key} has no row");
    }
}

#[tokio::test]
async fn the_postgres_secret_store_lists_each_secret_with_its_last_write() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let store = PgSecretStore::new(backend().await, keys());
    let key = SecretKey::new(&format!("wrkspc_list{}", stamp()), "vlt_a", "vcrd_a").unwrap();
    store.put(&key, &SecretString::from("one")).await.unwrap();
    let first = listed(&store).await.into_iter().find(|s| s.key == key).unwrap();
    store.put(&key, &SecretString::from("two")).await.unwrap();
    let second = listed(&store).await.into_iter().find(|s| s.key == key).unwrap();
    assert!(second.written_at > first.written_at, "an overwrite moves the write time");
    store.delete(&key).await.unwrap();
    assert!(!listed(&store).await.iter().any(|s| s.key == key));
}

/// Every page of `store`'s listing.
async fn listed(store: &PgSecretStore) -> Vec<Stored<SecretKey>> {
    let (mut all, mut after) = (vec![], None);
    loop {
        let page = store.list(after).await.unwrap();
        all.extend(page.items);
        after = page.next;
        if after.is_none() {
            return all;
        }
    }
}
