//! Session leases on Postgres. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use shared::ports::lease::Leases;
use store_postgres::PgLeases;

#[tokio::test]
async fn a_lease_has_one_holder_across_connections_until_dropped() {
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL").unwrap();
    let one = PgLeases::connect(&url, 4).await.unwrap();
    let other = PgLeases::connect(&url, 4).await.unwrap();
    let key = format!("session:test-{}", std::process::id());
    let held = one.try_acquire(&key).await.unwrap();
    assert!(held.is_some());
    assert!(other.try_acquire(&key).await.unwrap().is_none());
    drop(held);
    let mut retaken = None;
    for _ in 0..50 {
        retaken = other.try_acquire(&key).await.unwrap();
        if retaken.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(retaken.is_some(), "the unlock should free the lease");
}
