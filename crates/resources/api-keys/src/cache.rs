//! Workspace keys resolved within the last 60 s answer without reading
//! Postgres (D8): at most 10,000 per replica, by the key's SHA-256, never the
//! key itself, and only keys that resolved. A revoke anywhere (the API,
//! `agentd keys revoke`, a workspace's keys) reaches every replica through
//! the change feed: [`KeyRevokes`] publishes the key's id on `agentd:keys` and
//! each replica's cache drops it (SH-17). If that message is lost, the 60 s
//! bounds how long the key still works.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::Value;
use shared::hash::content_sha256;
use shared::ports::broadcast::Broadcast;
use shared::ports::changes::{Change, ChangeOp, NativeChangeHandler, Source};
use shared::ports::keys::{KeyError, KeySource};
use shared::ttl::TtlMap;
use shared::{AuthContext, BackendError};

/// How long a resolved key is kept.
const TTL: Duration = Duration::from_secs(60);
/// Keys kept per replica.
const KEYS: usize = 10_000;

/// Whether a lookup was answered from the cache: `hit` or `miss`.
pub type Report = Arc<dyn Fn(&'static str) + Send + Sync>;

/// A [`KeySource`] in front of another, keeping what it resolves.
pub struct CachedKeys {
    inner: Arc<dyn KeySource>,
    keys: TtlMap<String, AuthContext>,
    /// Raised by every revoke: a lookup that read Postgres before a revoke
    /// and finishes after it keeps nothing, so it cannot cache the revoked key.
    revokes: Mutex<u64>,
    report: Report,
}

impl std::fmt::Debug for CachedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedKeys").finish_non_exhaustive()
    }
}

impl CachedKeys {
    /// In front of `inner`, telling `report` of each hit and miss.
    pub fn new(inner: Arc<dyn KeySource>, report: Report) -> Self {
        Self { inner, keys: TtlMap::new(TTL, KEYS), revokes: Mutex::new(0), report }
    }

    /// Stop answering for the key whose id is `api_key_id`.
    pub fn revoke(&self, api_key_id: &str) {
        let mut revokes = self.revokes.lock().unwrap_or_else(PoisonError::into_inner);
        *revokes += 1;
        self.keys.remove_where(|ctx| ctx.api_key_id == api_key_id);
    }

    /// Revoke each key id `revoked` names, until it ends.
    pub fn listen(self: Arc<Self>, mut revoked: BoxStream<'static, String>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            while let Some(id) = revoked.next().await {
                self.revoke(&id);
            }
        })
    }

    fn revokes(&self) -> u64 {
        *self.revokes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl KeySource for CachedKeys {
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError> {
        let digest = content_sha256(key.as_bytes());
        if let Some(ctx) = self.keys.get(&digest) {
            (self.report)("hit");
            return Ok(Some(ctx));
        }
        (self.report)("miss");
        let before = self.revokes();
        let found = self.inner.resolve(key).await?;
        if let Some(ctx) = &found {
            let revokes = self.revokes.lock().unwrap_or_else(PoisonError::into_inner);
            if *revokes == before {
                self.keys.insert(digest, ctx.clone());
            }
        }
        Ok(found)
    }
}

/// The channel revoked key ids go on.
pub const REVOKED: &str = "agentd:keys";

/// Publishes each revoked key's id.
#[derive(Clone)]
pub struct KeyRevokes {
    broadcast: Arc<dyn Broadcast>,
}

impl std::fmt::Debug for KeyRevokes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyRevokes").finish_non_exhaustive()
    }
}

impl KeyRevokes {
    /// Publishing through `broadcast`.
    pub fn new(broadcast: Arc<dyn Broadcast>) -> Self {
        Self { broadcast }
    }
}

impl NativeChangeHandler for KeyRevokes {
    fn source(&self) -> Source {
        Source::Table("api_keys".into())
    }

    /// A failed publish is handled again, since a lost revoke would keep the key working for up to 60 s.
    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let Some(row) = change.after.as_ref().filter(|_| change.op == ChangeOp::Update) else {
            return Ok(());
        };
        let revoked = row.get("revoked_at").is_some_and(|r| !r.is_null());
        let Some(id) = row.get("api_key_id").and_then(Value::as_str).filter(|_| revoked) else {
            return Ok(());
        };
        self.broadcast.publish(REVOKED, id).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use shared::ports::broadcast::LocalBroadcast;
    use shared::ports::changes::ChangeHandler;
    use shared::{OrgId, WorkspaceId};

    use super::*;

    /// Counts its reads; `revoke_during` revokes through `cache` while a read is in flight.
    #[derive(Default)]
    struct Store {
        reads: AtomicUsize,
        during: Mutex<Option<Arc<CachedKeys>>>,
    }

    #[async_trait]
    impl KeySource for Store {
        async fn resolve(&self, _: &str) -> Result<Option<AuthContext>, KeyError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
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
}
