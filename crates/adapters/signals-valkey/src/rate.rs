//! The rate windows on Valkey: one script per request increments the
//! window's counter, starts its 60 s expiry on its first request, and
//! returns the count and what is left of the window (`agentd:rate:{org}:{bucket}`).

use std::time::Duration;

use redis::RedisError;
use shared::BackendError;
use shared::ports::rate::{Counted, NativeRateCounter, WINDOW};

use crate::valkey::{Shared, Valkey};

/// The longest a request waits on its count before it is admitted unchecked.
const WAIT: Duration = Duration::from_millis(200);

const HIT: &str = "local n = redis.call('INCR', KEYS[1]) \
                   if n == 1 then redis.call('PEXPIRE', KEYS[1], ARGV[1]) end \
                   return {n, redis.call('PTTL', KEYS[1])}";

/// [`RateCounter`](shared::ports::rate::RateCounter) over Valkey.
#[derive(Clone)]
pub struct ValkeyRateCounter {
    conn: Shared,
}

impl std::fmt::Debug for ValkeyRateCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValkeyRateCounter").finish_non_exhaustive()
    }
}

impl ValkeyRateCounter {
    /// Over `valkey`; nothing connects until first used.
    pub fn connect(valkey: &Valkey) -> Result<Self, RedisError> {
        Ok(Self { conn: valkey.shared()? })
    }
}

impl NativeRateCounter for ValkeyRateCounter {
    async fn hit(&self, org: &str, bucket: &str) -> Result<Counted, BackendError> {
        let key = format!("agentd:rate:{org}:{bucket}");
        let window = u64::try_from(WINDOW.as_millis()).unwrap_or(u64::MAX);
        let script = redis::cmd("EVAL").arg(HIT).arg(1).arg(&key).arg(window).clone();
        let counted = tokio::time::timeout(WAIT, async {
            script.query_async::<(u64, i64)>(&mut self.conn.get().await?).await
        });
        let (count, ttl) = counted.await.map_err(BackendError::new)?.map_err(BackendError::new)?;
        let resets_in = Duration::from_millis(u64::try_from(ttl).unwrap_or(0));
        Ok(Counted { count, resets_in })
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn requests_share_a_fixed_window_that_starts_at_the_first() {
        for valkey in crate::test_valkeys() {
            let rate = ValkeyRateCounter::connect(&valkey).unwrap();
            let org = format!("org_{}", uuid::Uuid::new_v4());
            let first = rate.hit(&org, "read").await.unwrap();
            let second = rate.hit(&org, "read").await.unwrap();
            assert_eq!((first.count, second.count), (1, 2));
            assert!(second.resets_in <= first.resets_in && first.resets_in <= WINDOW);
            assert_eq!(rate.hit(&org, "create").await.unwrap().count, 1, "each bucket counts apart");
        }
    }

    #[tokio::test]
    async fn with_valkey_down_a_hit_fails_within_its_wait() {
        let rate = ValkeyRateCounter::connect(&Valkey::open("redis://127.0.0.1:1", false).unwrap()).unwrap();
        let started = tokio::time::Instant::now();
        assert!(rate.hit("org", "read").await.is_err());
        assert!(started.elapsed() < WAIT * 3);
    }
}
