//! An ordered stream's lease: one replica reads it at a time. Taking it is
//! `SET NX PX`; extending it is one script that extends only the holder's own
//! lease, so a replica that lost it learns so instead of extending another's.
//! While an entry runs, a ticker extends it every 10 s, a third of its 30 s
//! (Redis, "Distributed locks with Redis").

use std::future::Future;
use std::time::Duration;

use redis::{AsyncCommands, RedisResult, SetExpiry, SetOptions};

use super::LEASE_MS;
use crate::valkey::{Conn, Shared};

/// How often a running entry's lease is extended.
const EXTEND_EVERY: Duration = Duration::from_millis(LEASE_MS / 3);

const EXTEND: &str = "if redis.call('GET', KEYS[1]) == ARGV[1] then \
                      return redis.call('PEXPIRE', KEYS[1], ARGV[2]) else return 0 end";

/// One replica's hold on one stream.
pub(super) struct Lease<'a> {
    pub(super) key: String,
    pub(super) me: &'a str,
}

impl Lease<'_> {
    /// Take the lease, or extend it when this replica holds it; `false` when another does.
    pub(super) async fn take(&self, conn: &mut Conn) -> RedisResult<bool> {
        if self.extend(conn).await? {
            return Ok(true);
        }
        let options = SetOptions::default()
            .conditional_set(redis::ExistenceCheck::NX)
            .with_expiration(SetExpiry::PX(LEASE_MS));
        let taken: Option<String> = conn.set_options(&self.key, self.me, options).await?;
        Ok(taken.is_some())
    }

    /// Extend the lease this replica holds; `false` when it no longer does.
    pub(super) async fn extend<C: redis::aio::ConnectionLike + Send>(
        &self,
        conn: &mut C,
    ) -> RedisResult<bool> {
        let script = redis::cmd("EVAL").arg(EXTEND).arg(1).arg(&self.key).arg(self.me).arg(LEASE_MS).clone();
        let extended: i64 = script.query_async(conn).await?;
        Ok(extended == 1)
    }

    /// Run `entry` while extending the lease every 10 s on `ticker`; `None`
    /// when the lease was lost first, and the entry is dropped unfinished.
    pub(super) async fn hold<T>(&self, ticker: &Shared, entry: impl Future<Output = T>) -> Option<T> {
        let kept = async {
            let mut every = tokio::time::interval(EXTEND_EVERY);
            every.tick().await;
            let mut extended = tokio::time::Instant::now();
            // Unanswered for a whole lease, it may have passed to another replica: count it lost.
            while extended.elapsed() < Duration::from_millis(LEASE_MS) {
                every.tick().await;
                let answer = match ticker.get().await {
                    Ok(mut conn) => self.extend(&mut conn).await,
                    Err(e) => Err(e),
                };
                match answer {
                    Ok(true) => extended = tokio::time::Instant::now(),
                    Ok(false) => return,
                    Err(e) => {
                        tracing::warn!(lease = %self.key, error = %e, "changes: the lease could not be extended")
                    }
                }
            }
        };
        tokio::select! {
            done = entry => Some(done),
            () = kept => None,
        }
    }
}
