//! Organization admin keys over live Postgres: only the digest is stored, a
//! key resolves to its organization until it is revoked, and a workspace key
//! is not an admin key. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use ::api_keys::AdminKeysService;
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::Timestamp;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::{AdminKeySource, KeyPepper, PepperRing};
use shared::ports::secrets::ExposeSecret;
use store_postgres::schema::admin_keys;
use store_postgres::{PgAdminKeysStore, PgBackend};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

#[tokio::test]
async fn an_admin_key_resolves_to_its_org_until_it_is_revoked_and_only_its_digest_is_stored() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let peppers = Arc::new(PepperRing::single(KeyPepper::new(&[5; 32])));
    let keys = AdminKeysService::new(Arc::new(PgAdminKeysStore::new(pg.clone())), peppers);
    let org = format!("org_admin{}", Timestamp::now().as_datetime().timestamp_micros());
    let (record, key) = keys.create(&org, "ops").await.unwrap();
    assert!(record.id.starts_with("adminkey_01") && key.expose_secret().starts_with("sk-ant-admin01-"));
    let ctx = keys.resolve(key.expose_secret()).await.unwrap().unwrap();
    assert_eq!((ctx.org.as_str(), ctx.admin_key_id.as_str()), (org.as_str(), record.id.as_str()));

    let mut conn = pg.diesel.get().await.unwrap();
    let digest: Vec<u8> = admin_keys::table
        .filter(admin_keys::admin_key_id.eq(&record.id))
        .select(admin_keys::key_digest)
        .first(&mut conn)
        .await
        .unwrap();
    assert_eq!(digest.len(), 32);
    assert!(!String::from_utf8_lossy(&digest).contains(&key.expose_secret()[15..]));

    let workspace = material::generate(KeyKind::Workspace);
    assert!(keys.resolve(workspace.expose_secret()).await.unwrap().is_none());
    assert!(keys.list().await.unwrap().iter().any(|k| k.id == record.id && k.revoked_at.is_none()));
    assert!(keys.revoke(&record.id).await.unwrap());
    assert!(keys.resolve(key.expose_secret()).await.unwrap().is_none());
    assert!(!keys.revoke(&record.id).await.unwrap());
}
