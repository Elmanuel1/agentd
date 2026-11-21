use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use shared::ports::broadcast::LocalBroadcast;
use shared::ports::changes::ChangeHandler;
use shared::{OrgId, WorkspaceId};

use super::*;

/// Counts its reads, each taking `wait`; `during` revokes through its cache while a read is in flight.
#[derive(Default)]
struct Store {
    reads: AtomicUsize,
    during: Mutex<Option<Arc<CachedKeys>>>,
    wait: Mutex<Duration>,
}

#[async_trait]
impl KeySource for Store {
    async fn resolve(&self, _: &str) -> Result<Option<AuthContext>, KeyError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let wait = *self.wait.lock().unwrap();
        tokio::time::sleep(wait).await;
        let during = self.during.lock().unwrap().take();
        if let Some(cache) = during {
            cache.revoke("apikey_1");
        }
        Ok(Some(AuthContext {
            org: OrgId::new("org_1"),
            workspace: WorkspaceId::new("wrkspc_1"),
            api_key_id: "apikey_1".into(),
        }))
    }
}

fn cached(store: &Arc<Store>) -> Arc<CachedKeys> {
    Arc::new(CachedKeys::new(store.clone(), Arc::new(|_| {})))
}

#[tokio::test(start_paused = true)]
async fn a_cached_key_reads_postgres_once_until_its_ttl_or_its_revoke() {
    let store = Arc::new(Store::default());
    let cache = cached(&store);
    for _ in 0..3 {
        assert!(cache.resolve("sk-ant-api03-k").await.unwrap().is_some());
    }
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    tokio::time::advance(TTL).await;
    cache.resolve("sk-ant-api03-k").await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 2, "read again after the TTL");
    cache.revoke("apikey_1");
    cache.resolve("sk-ant-api03-k").await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 3, "read again after a revoke");
}

#[tokio::test]
async fn a_miss_that_reads_a_key_while_it_is_revoked_does_not_cache_it() {
    let store = Arc::new(Store::default());
    let cache = cached(&store);
    *store.during.lock().unwrap() = Some(cache.clone());
    cache.resolve("sk-ant-api03-k").await.unwrap();
    cache.resolve("sk-ant-api03-k").await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 2, "the first read was not kept");
}

#[tokio::test]
async fn a_revoke_on_the_feed_reaches_the_cache_within_a_second_and_other_updates_do_not() {
    let store = Arc::new(Store::default());
    let cache = cached(&store);
    let local = Arc::new(LocalBroadcast::default());
    let _listening = cache.clone().listen(Broadcast::subscribe(local.as_ref(), REVOKED));
    let handler = KeyRevokes::new(local);
    let change = |revoked: Value| Change {
        table: "api_keys".into(),
        op: ChangeOp::Update,
        position: None,
        before: None,
        after: Some(json!({"api_key_id": "apikey_1", "revoked_at": revoked})),
    };
    cache.resolve("sk-ant-api03-k").await.unwrap();
    ChangeHandler::handle(&handler, &change(Value::Null)).await.unwrap();
    tokio::task::yield_now().await;
    cache.resolve("sk-ant-api03-k").await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 1, "an update that revokes nothing keeps the key");
    ChangeHandler::handle(&handler, &change(json!("2026-11-19T00:00:00Z"))).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while store.reads.load(Ordering::SeqCst) == 1 {
            cache.resolve("sk-ant-api03-k").await.unwrap();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the revoke reached the cache within 1 s");
}

#[tokio::test(start_paused = true)]
async fn a_key_s_60_s_count_from_the_start_of_the_read_that_filled_it() {
    // SH-18: a read that took 30 s keeps the key for 30 s more, not 60.
    let store = Arc::new(Store::default());
    let cache = cached(&store);
    *store.wait.lock().unwrap() = Duration::from_secs(30);
    cache.resolve("sk-ant-api03-k").await.unwrap();
    *store.wait.lock().unwrap() = Duration::ZERO;
    tokio::time::advance(Duration::from_secs(29)).await;
    cache.resolve("sk-ant-api03-k").await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 1, "59 s after the read began: cached");
    tokio::time::advance(Duration::from_secs(1)).await;
    cache.resolve("sk-ant-api03-k").await.unwrap();
    assert_eq!(store.reads.load(Ordering::SeqCst), 2, "60 s after: read again");
}
