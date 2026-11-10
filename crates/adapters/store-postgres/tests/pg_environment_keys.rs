//! Environment keys over live Postgres: only the peppered digest is stored,
//! bound to one organization, workspace and environment, and a key resolves
//! until it is revoked. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;

use ::api_keys::EnvironmentKeysService;
use diesel::{ExpressionMethods, QueryDsl};
use diesel_async::RunQueryDsl;
use shared::ports::keys::{EnvironmentGrant, EnvironmentKeySource, KeyPepper};
use shared::ports::secrets::ExposeSecret;
use shared::Timestamp;
use store_postgres::schema::environment_keys;
use store_postgres::{PgBackend, PgEnvironmentKeysStore};

#[tokio::test]
async fn only_the_digest_is_stored_and_a_key_resolves_to_its_environment_until_revoked() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    let pg = PgBackend::connect(&url).await.unwrap();
    let keys = EnvironmentKeysService::new(
        Arc::new(PgEnvironmentKeysStore::new(pg.clone())),
        Arc::new(KeyPepper::new(&[7; 32])),
    );
    let s = Timestamp::now().as_datetime().timestamp_micros();
    let (ws, env) = (format!("wrkspc_e{s}"), format!("env_e{s}"));
    let (record, key) = keys.create("org_env", &ws, &env, "worker").await.unwrap();
    let key = key.expose_secret();
    assert!(key.starts_with("sk-ant-oat01-"));
    let mut conn = pg.diesel.get().await.unwrap();
    let (digest, hint, stored_env): (Vec<u8>, String, String) = environment_keys::table
        .filter(environment_keys::environment_key_id.eq(&record.id))
        .select((
            environment_keys::key_digest,
            environment_keys::partial_key_hint,
            environment_keys::environment_id,
        ))
        .first(&mut conn)
        .await
        .unwrap();
    assert_eq!((digest.len(), stored_env.as_str()), (32, env.as_str()));
    assert!(
        !hint.contains(&key[20..90])
            && !digest.windows(16).any(|w| key.as_bytes().windows(16).any(|k| k == w))
    );
    let Some(EnvironmentGrant::Environment(ctx, caller)) = keys.resolve(key).await.unwrap() else {
        panic!("resolves")
    };
    assert_eq!(
        (ctx.org.as_str(), ctx.workspace.as_str(), caller.queue.as_str()),
        ("org_env", ws.as_str(), env.as_str())
    );
    assert!(keys.revoke(&record.id).await.unwrap());
    assert_eq!(keys.resolve(key).await.unwrap(), None);
}
