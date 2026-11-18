//! The slot and history metrics' source: the change feed's slot as Postgres
//! reports it. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`; the live slot
//! also needs `AGENTD_CDC_TESTS=1` and a running Debezium Server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use shared::Timestamp;
use store_postgres::{PgBackend, read_slot};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

#[tokio::test]
async fn the_feed_s_live_slot_reads_as_kept_with_the_wal_it_holds() {
    if !enabled() || std::env::var("AGENTD_CDC_TESTS").as_deref() != Ok("1") {
        return;
    }
    let pg = backend().await;
    let slot = read_slot(&pg.diesel, "agentd_changes").await.unwrap();
    assert!(!slot.lost, "{slot:?}");
    assert!(slot.retained.is_some_and(|r| r >= 0), "{slot:?}");
    assert!(slot.wal_written > 0, "{slot:?}");
}

#[tokio::test]
async fn a_dropped_slot_reads_as_lost() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let name = format!("feed_metrics_{}", Timestamp::now().as_datetime().timestamp_micros());
    let mut conn = pg.diesel.get().await.unwrap();
    // A physical slot that reserves WAL needs no logical WAL level.
    diesel::sql_query("SELECT 1 FROM pg_create_physical_replication_slot($1, true)")
        .bind::<Text, _>(&name)
        .execute(&mut *conn)
        .await
        .unwrap();
    let held = read_slot(&pg.diesel, &name).await.unwrap();
    assert!(!held.lost && held.retained.is_some(), "{held:?}");
    diesel::sql_query("SELECT pg_drop_replication_slot($1)")
        .bind::<Text, _>(&name)
        .execute(&mut *conn)
        .await
        .unwrap();
    let gone = read_slot(&pg.diesel, &name).await.unwrap();
    assert_eq!((gone.lost, gone.retained, gone.safe), (true, None, None));
}
