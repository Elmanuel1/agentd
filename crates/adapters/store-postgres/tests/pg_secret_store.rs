//! The Postgres secret store over live Postgres: its three operations, one
//! statement each, the envelope as stored, rotation and `reseal`, and the
//! table kept off the change feed. Gated by `AGENTD_PG_TESTS=1` +
//! `DATABASE_URL`, on a database Flyway migrated.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use diesel::connection::{Instrumentation, InstrumentationEvent};
use diesel::sql_types::{Bool, Bytea, Text};
use diesel::QueryableByName;
use diesel_async::RunQueryDsl;
use shared::hash::SecretKeys;
use shared::ports::secrets::{ExposeSecret, SecretError, SecretKey, SecretStore, SecretString};
use shared::Timestamp;
use store_postgres::{PgBackend, PgObserver, PgSecretStore, PoolSettings};

const K1: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const K2: &str = "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";

fn url() -> Option<String> {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return None;
    }
    Some(
        std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into()),
    )
}

async fn store(url: &str, ring: &str) -> PgSecretStore {
    PgSecretStore::new(PgBackend::connect(url).await.unwrap(), SecretKeys::parse(ring).unwrap())
}

/// A workspace no other test or run writes to.
fn workspace(tag: &str) -> String {
    format!("wrkspc_{tag}{}", Timestamp::now().as_datetime().timestamp_micros())
}

fn secret(value: &str) -> SecretString {
    SecretString::from(value)
}

#[derive(QueryableByName, Debug)]
struct Stored {
    #[diesel(sql_type = Bool)]
    environment_variable: bool,
    #[diesel(sql_type = Text)]
    key_id: String,
    #[diesel(sql_type = Bytea)]
    sealed_key: Vec<u8>,
    #[diesel(sql_type = Bytea)]
    sealed_value: Vec<u8>,
}

async fn stored(url: &str, key: &SecretKey) -> Option<Stored> {
    let pg = PgBackend::connect(url).await.unwrap();
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query(
        "SELECT environment_variable, key_id, sealed_key, sealed_value FROM vault_secrets
         WHERE workspace_id = $1 AND vault_id = $2 AND credential_id = $3",
    )
    .bind::<Text, _>(key.workspace())
    .bind::<Text, _>(key.vault())
    .bind::<Text, _>(key.credential())
    .get_results(&mut conn)
    .await
    .unwrap()
    .pop()
}

async fn run(url: &str, sql: &str) {
    let pg = PgBackend::connect(url).await.unwrap();
    diesel::sql_query(sql).execute(&mut pg.diesel.get().await.unwrap()).await.unwrap();
}

/// What runs before the first statement containing its text is sent.
type Hook = Option<(String, Box<dyn FnOnce() + Send>)>;

/// Every statement a pool runs, and a hook.
#[derive(Clone, Default)]
struct Seen {
    statements: Arc<Mutex<Vec<String>>>,
    hook: Arc<Mutex<Hook>>,
}

impl PgObserver for Seen {
    fn statements(&self) -> Box<dyn Instrumentation> {
        Box::new(self.clone())
    }

    fn connect_failed(&self, _: &diesel::ConnectionError) {}
}

impl Instrumentation for Seen {
    fn on_connection_event(&mut self, event: InstrumentationEvent<'_>) {
        let InstrumentationEvent::StartQuery { query, .. } = event else { return };
        let query = query.to_string();
        let mut hook = self.hook.lock().unwrap();
        if hook.as_ref().is_some_and(|(trigger, _)| query.contains(trigger.as_str())) {
            if let Some((_, run)) = hook.take() {
                run();
            }
        }
        self.statements.lock().unwrap().push(query);
    }
}

#[tokio::test]
async fn a_secret_is_put_read_overwritten_and_deleted_and_a_missing_one_is_not_found() {
    let Some(url) = url() else { return };
    let store = store(&url, &format!("k1:{K1}")).await;
    let key = SecretKey::new(&workspace("crud"), "vlt_01a", "cred_01a").unwrap();
    assert!(matches!(store.get(&key).await, Err(SecretError::NotFound(_))));
    store.delete(&key).await.unwrap();
    store.put(&key, &secret("one")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "one");
    store.put(&key, &secret("two")).await.unwrap();
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "two");
    store.delete(&key).await.unwrap();
    assert!(matches!(store.get(&key).await, Err(SecretError::NotFound(_))));
    store.delete(&key).await.unwrap();
}

#[tokio::test]
async fn put_get_and_delete_are_one_statement_each() {
    let Some(url) = url() else { return };
    let seen = Seen::default();
    let statements = seen.statements.clone();
    let pg = PgBackend::connect_observed(&url, PoolSettings::default(), Arc::new(seen)).await.unwrap();
    let store = PgSecretStore::new(pg, SecretKeys::parse(&format!("k1:{K1}")).unwrap());
    let key = SecretKey::new(&workspace("count"), "vlt_01a", "cred_01a").unwrap();
    let take = || std::mem::take(&mut *statements.lock().unwrap());
    store.put(&key, &secret("v")).await.unwrap();
    let put = take();
    assert!(put.len() == 1 && put[0].starts_with("INSERT INTO \"vault_secrets\""), "{put:?}");
    store.get(&key).await.unwrap();
    let get = take();
    assert!(get.len() == 1 && get[0].starts_with("SELECT"), "{get:?}");
    store.delete(&key).await.unwrap();
    let delete = take();
    assert!(delete.len() == 1 && delete[0].starts_with("DELETE FROM \"vault_secrets\""), "{delete:?}");
}

#[tokio::test]
async fn the_kind_is_read_from_the_row_and_bound_to_it() {
    let Some(url) = url() else { return };
    let store = store(&url, &format!("k1:{K1}")).await;
    let ws = workspace("kind");
    let key = SecretKey::new(&ws, "vlt_01a", "cred_01a").unwrap();
    store.put(&key.clone().environment_variable(), &secret("env")).await.unwrap();
    assert!(stored(&url, &key).await.unwrap().environment_variable);
    assert_eq!(store.get(&key).await.unwrap().expose_secret(), "env", "read with a plain key");
    run(&url, &format!("UPDATE vault_secrets SET environment_variable = false WHERE workspace_id = '{ws}'"))
        .await;
    let err = store.get(&key).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("secret backend: vault secret {key} does not open with key k1"),
        "a flipped kind does not open"
    );
    store.delete(&key).await.unwrap();
    assert!(stored(&url, &key).await.is_none(), "deleted with a plain key");
}

#[tokio::test]
async fn the_row_holds_no_value_or_key_bytes_and_names_the_first_key() {
    let Some(url) = url() else { return };
    let store = store(&url, &format!("k2:{K2},k1:{K1}")).await;
    let key = SecretKey::new(&workspace("bytes"), "vlt_01a", "cred_01a").unwrap();
    let value = "known-40-byte-secret-value-0123456789abc";
    assert_eq!(value.len(), 40);
    store.put(&key, &secret(value)).await.unwrap();
    let row = stored(&url, &key).await.unwrap();
    assert_eq!(row.key_id, "k2");
    let (k1, k2) = (hex::decode(K1).unwrap(), hex::decode(K2).unwrap());
    for needle in [value.as_bytes(), &k1, &k2] {
        for window in needle.windows(8) {
            for bytes in [&row.sealed_key, &row.sealed_value] {
                assert!(!bytes.windows(8).any(|w| w == window), "a stored column holds known bytes");
            }
        }
    }
    store.delete(&key).await.unwrap();
}

#[tokio::test]
async fn rotation_reopens_old_rows_and_reseal_moves_the_rest_past_a_bad_row_and_a_newer_put() {
    let Some(url) = url() else { return };
    let (old, rotated) = (format!("r1:{K1}"), format!("r2:{K2},r1:{K1}"));
    let ws = workspace("reseal");
    let key = |c: &str| SecretKey::new(&ws, "vlt_01a", c).unwrap();
    let (moved, raced, reput, lost) = (key("cred_4"), key("cred_2"), key("cred_1"), key("cred_3"));
    let first = store(&url, &old).await;
    for (k, v) in [(&moved, "v4"), (&raced, "v2"), (&reput, "v1")] {
        first.put(k, &secret(v)).await.unwrap();
    }
    store(&url, &format!("z0:{K2}")).await.put(&lost, &secret("v3")).await.unwrap();
    // Sorted first, with a credential id outside the id charset: no key, so reseal passes over it.
    run(
        &url,
        &format!(
            "INSERT INTO vault_secrets VALUES ('{ws}', 'vlt_01a', 'cred-0', false, 'r1',
             decode(repeat('00', 92), 'hex'), decode(repeat('00', 28), 'hex'))"
        ),
    )
    .await;
    let before = stored(&url, &moved).await.unwrap();

    let next = store(&url, &rotated).await;
    assert_eq!(next.get(&reput).await.unwrap().expose_secret(), "v1", "the old key still opens");
    next.put(&reput, &secret("v1b")).await.unwrap();
    assert_eq!(stored(&url, &reput).await.unwrap().key_id, "r2", "the next put moves it");

    let seen = Seen::default();
    let put_first = (url.clone(), rotated.clone(), raced.clone());
    *seen.hook.lock().unwrap() = Some((
        format!("\"{ws}\", \"vlt_01a\", \"{}\"", raced.credential()),
        Box::new(move || {
            let (url, ring, key) = put_first;
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(async { store(&url, &ring).await.put(&key, &secret("newer")).await.unwrap() });
            })
            .join()
            .unwrap();
        }),
    ));
    let watched = PgBackend::connect_observed(&url, PoolSettings::default(), Arc::new(seen.clone())).await;
    let resealer = PgSecretStore::new(watched.unwrap(), SecretKeys::parse(&rotated).unwrap());
    let report = resealer.reseal().await.unwrap();
    assert!(seen.hook.lock().unwrap().is_none(), "the put ran between reseal's read and its write");

    let after = stored(&url, &moved).await.unwrap();
    assert_eq!(after.key_id, "r2");
    assert_ne!(after.sealed_key, before.sealed_key, "a new data key");
    assert_ne!(after.sealed_value, before.sealed_value, "the value sealed again");
    assert_eq!(next.get(&moved).await.unwrap().expose_secret(), "v4");
    assert_eq!(next.get(&raced).await.unwrap().expose_secret(), "newer", "the newer put is kept");
    assert!(report.moved >= 1, "{report:?}");
    let ours: Vec<_> = report.remaining.iter().filter(|(k, _)| k.starts_with(&ws)).cloned().collect();
    assert_eq!(
        ours,
        [(format!("{ws}/vlt_01a/cred-0"), "r1".to_owned()), (lost.to_string(), "z0".to_owned())],
        "only the row that is not a key and the row under a key the ring lacks remain"
    );
    run(&url, &format!("DELETE FROM vault_secrets WHERE workspace_id = '{ws}'")).await;
}

#[tokio::test]
async fn vault_secrets_is_off_the_change_feed_and_history() {
    let Some(url) = url() else { return };
    #[derive(QueryableByName)]
    struct Count {
        #[diesel(sql_type = diesel::sql_types::BigInt)]
        n: i64,
    }
    let pg = PgBackend::connect(&url).await.unwrap();
    let triggers: Count =
        diesel::sql_query("SELECT count(*) AS n FROM pg_trigger WHERE tgrelid = 'vault_secrets'::regclass")
            .get_result(&mut pg.diesel.get().await.unwrap())
            .await
            .unwrap();
    assert_eq!(triggers.n, 0, "no history stamp or delete trigger");
    let properties = include_str!("../../../../docker/debezium/application.properties");
    let include = properties.lines().find(|l| l.starts_with("debezium.source.table.include.list=")).unwrap();
    assert!(!include.contains("vault_secrets"), "{include}");
}
