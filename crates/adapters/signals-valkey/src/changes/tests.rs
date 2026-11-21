//! The lease and the unordered pass against the test Valkeys, on streams of their own.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::*;

fn name() -> String {
    format!("agentd.test.{}", uuid::Uuid::new_v4())
}

#[tokio::test]
async fn a_lease_taken_over_mid_entry_stops_the_old_holder() {
    for valkey in crate::test_valkeys() {
        let mut conn = valkey.connection().await.unwrap();
        let key = format!("agentd:lease:{}", name());
        let (a, b) = (lease::Lease { key: key.clone(), me: "a" }, lease::Lease { key: key.clone(), me: "b" });
        assert!(a.take(&mut conn).await.unwrap());
        assert!(!b.take(&mut conn).await.unwrap(), "held by a");
        let _: () = conn.set(&key, "b").await.unwrap();
        assert!(!a.extend(&mut conn).await.unwrap(), "a does not extend b's lease");
        let started = tokio::time::Instant::now();
        let held = a.hold(&valkey.shared().unwrap(), std::future::pending::<()>()).await;
        assert_eq!(held, None, "the entry is dropped once the lease is lost");
        assert!(started.elapsed() < Duration::from_millis(LEASE_MS / 2));
        let holder: String = conn.get(&key).await.unwrap();
        assert_eq!(holder, "b");
    }
}

#[tokio::test]
async fn an_entry_longer_than_the_lease_keeps_it() {
    for valkey in crate::test_valkeys() {
        let mut conn = valkey.connection().await.unwrap();
        let lease = lease::Lease { key: format!("agentd:lease:{}", name()), me: "a" };
        assert!(lease.take(&mut conn).await.unwrap());
        let long = tokio::time::sleep(Duration::from_millis(LEASE_MS + 5_000));
        assert_eq!(lease.hold(&valkey.shared().unwrap(), long).await, Some(()));
        let holder: Option<String> = conn.get(&lease.key).await.unwrap();
        assert_eq!(holder.as_deref(), Some("a"), "extended while it ran");
    }
}

/// Handles an entry in 2 s, counting each.
#[derive(Default)]
struct Slow(AtomicUsize);

impl NativeConsumer for Slow {
    fn ordered(&self) -> bool {
        false
    }

    async fn consume(&self, _: &StreamId) -> bool {
        tokio::time::sleep(Duration::from_secs(2)).await;
        self.0.fetch_add(1, Ordering::SeqCst);
        true
    }
}

#[tokio::test]
async fn a_slow_unordered_batch_is_handled_together_and_once() {
    for valkey in crate::test_valkeys() {
        let stream = name();
        let mut conn = valkey.connection().await.unwrap();
        for n in 0..unordered::UNORDERED_BATCH {
            let _: String = conn.xadd(&stream, "*", &[("value", n.to_string())]).await.unwrap();
        }
        let _: () = conn.xgroup_create(&stream, GROUP, "0").await.unwrap();
        let changes = ValkeyChanges::new(&valkey, "agentd", "a");
        let slow = Slow::default();
        let started = tokio::time::Instant::now();
        changes.pass(&mut conn, &stream, &slow).await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(4), "handled together: {:?}", started.elapsed());
        assert_eq!(slow.0.load(Ordering::SeqCst), unordered::UNORDERED_BATCH);
        let pending: redis::streams::StreamPendingReply = conn.xpending(&stream, GROUP).await.unwrap();
        assert_eq!(pending.count(), 0, "every entry acknowledged");
        changes.pass(&mut conn, &stream, &slow).await.unwrap();
        assert_eq!(slow.0.load(Ordering::SeqCst), unordered::UNORDERED_BATCH, "none handled twice");
        let _: () = conn.del(&stream).await.unwrap();
    }
}
