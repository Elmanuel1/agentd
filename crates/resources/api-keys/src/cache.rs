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
use tokio::time::Instant;

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
        // The TTL counts from the read's start, so a read in flight during a lost revoke still expires
        // within 60 s of it (SH-18).
        let (before, started) = (self.revokes(), Instant::now());
        let found = self.inner.resolve(key).await?;
        // Held through the insert, so a revoke cannot land between the check and the insert.
        let revokes = self.revokes.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(ctx) = found.as_ref().filter(|_| *revokes == before) {
            self.keys.insert_since(digest, ctx.clone(), started);
        }
        drop(revokes);
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
    pub fn new(broadcast: Arc<dyn Broadcast>) -> Self {
        Self { broadcast }
    }
}

impl NativeChangeHandler for KeyRevokes {
    fn source(&self) -> Source {
        Source::Table("api_keys".into())
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        // A failed publish is handled again: a lost revoke keeps the key working for up to 60 s.
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
mod tests;
