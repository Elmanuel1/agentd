//! Resource history through faults at each boundary on its way, Postgres's
//! slot to Debezium Server, Debezium Server to the broker, the broker to the
//! recorders, and the recorders to Postgres: each side is stopped, crashed,
//! hung or cut while changes are made, and every change, those made during
//! the fault included, reaches history exactly once, and a row's history read in lsn order
//! gives its changes in their commit order. Gated like `history.rs`, and by
//! `AGENTD_FAULT_TESTS=1` because it stops shared containers: Postgres
//! (`AGENTD_POSTGRES_CONTAINER`, `postgres`), Debezium Server
//! (`AGENTD_DEBEZIUM_CONTAINER`, `debezium`) and the feed's broker
//! (`AGENTD_FEED_CONTAINER`, `kafka`). Debezium Server runs as production
//! runs it, restarted when it stops. One test, so the faults never overlap.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;
#[path = "history/kit.rs"]
mod kit;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::StatusCode;
use common::Rig;
use diesel_async::RunQueryDsl;
use kit::{call, cdc_enabled, eventually, summary, Container};
use serde_json::json;
use shared::ports::changes::{Change, ChangeHandler, NativeChangeHandler, Source};
use shared::{BackendError, Timestamp};
use tokio::sync::Notify;

/// One history row as `summary` gives it.
type Kept = (String, String, String);

/// The consumer the recorders read the feed as, the same after agentd restarts.
const CONSUMER: &str = "agentd-faults";

/// How long a short outage lasts: past Debezium's and the consumers' first
/// retries, within the minute after which Debezium Server stops.
const OUTAGE: Duration = Duration::from_secs(20);

/// A broker outage long enough that Debezium Server's sink fails.
const LONG_OUTAGE: Duration = Duration::from_secs(90);

/// What Debezium Server logs when its sink gives up: on Kafka the producer
/// fails and the connector restarts, or the engine fails and the server stops
/// (and is started again); the Redis sink cannot reach Valkey and retries.
const SINK_FAILED: [&str; 3] = ["Going to restart connector", "Engine has failed", "Can't connect to Redis"];

/// The changes made while a side is down.
const MISSED: usize = 5;

/// Update vault `v` through the API, retrying a failed call as a client
/// would: after a Postgres restart, each pooled connection fails its first
/// statement (`RecyclingMethod::Fast`). The row it leaves.
async fn update(rig: &Rig, v: &str) -> Kept {
    let body = json!({"display_name": common::stamp()});
    for _ in 0..30 {
        let (status, _, id) = call(rig, "POST", &format!("/v1/vaults/{v}"), Some(body.clone())).await;
        if status == StatusCode::OK {
            return ("update".into(), "apikey_01a".into(), id);
        }
        assert!(status.is_server_error(), "{status}");
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("vault {v} could not be updated");
}

/// Where a recorder stalled, shared by the recorders before and after agentd restarts.
#[derive(Default)]
struct Stalled {
    /// The next vault change stalls its recorder.
    armed: AtomicBool,
    /// Its recorder stored it and stalled.
    stored: Notify,
    /// Its position.
    at: Mutex<Option<u64>>,
    /// A recorder handled it again.
    again: Notify,
}

/// A recorder that, once armed, stops after it stores a vault change and
/// before the feed learns it was handled, as a replica that dies between the
/// two would; and that notes when it handles the stalled change again.
struct Stall {
    inner: Arc<dyn ChangeHandler>,
    stalled: Arc<Stalled>,
}

impl NativeChangeHandler for Stall {
    fn source(&self) -> Source {
        self.inner.source()
    }

    fn ordered(&self) -> bool {
        self.inner.ordered()
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        self.inner.handle(change).await?;
        let lsn = change.position.map(|p| p.lsn);
        if change.table != "vaults" || lsn.is_none() {
            return Ok(());
        }
        if self.stalled.armed.swap(false, Ordering::SeqCst) {
            *self.stalled.at.lock().unwrap() = lsn;
            self.stalled.stored.notify_one();
            std::future::pending::<()>().await;
        }
        if *self.stalled.at.lock().unwrap() == lsn {
            self.stalled.again.notify_one();
        }
        Ok(())
    }
}

/// The recorders of every history table, each a [`Stall`] sharing `stalled`.
async fn stalling(stalled: &Arc<Stalled>) -> Vec<Arc<dyn ChangeHandler>> {
    let recorders = kit::recorders().await.into_iter();
    recorders
        .map(|inner| Arc::new(Stall { inner, stalled: stalled.clone() }) as Arc<dyn ChangeHandler>)
        .collect()
}

/// Wait out a broker outage of at least `LONG_OUTAGE` until Debezium Server
/// has logged its sink's failure since `since` (Unix seconds), at most 3 minutes.
async fn sink_failed(debezium: &Container, since: i64) {
    let started = std::time::Instant::now();
    loop {
        let logs = debezium.logs_since(since);
        let failed = SINK_FAILED.iter().any(|signal| logs.contains(signal));
        if failed && started.elapsed() >= LONG_OUTAGE {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(180), "Debezium Server's sink never failed");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// `n` updates of vault `v`, the rows they leave.
async fn updates(rig: &Rig, v: &str, n: usize) -> Vec<Kept> {
    let mut rows = Vec::with_capacity(n);
    for _ in 0..n {
        rows.push(update(rig, v).await);
    }
    rows
}

/// Vault `v`'s history is `expected`: nothing lost, nothing twice.
async fn kept_once(rig: &Rig, v: &str, expected: &[Kept], fault: &str) {
    let rows = eventually(rig, "vaults", "vault_id", v, expected.len()).await;
    assert_eq!(summary(&rows), expected, "after {fault}");
}

/// Stop the recorders as a stopping agentd would: abort each task and wait until it is
/// gone. A Kafka consumer leaves its group as it drops, blocking the thread until it has,
/// so a connection opened before the drops finish would time out.
async fn stop(tasks: Vec<tokio::task::JoinHandle<()>>) {
    tasks.iter().for_each(tokio::task::JoinHandle::abort);
    for task in tasks {
        let _ = task.await;
    }
}

/// Until Postgres answers on a fresh connection.
async fn postgres_back(rig: &Rig) {
    for _ in 0..120 {
        if let Ok(mut conn) = rig.pg.diesel.get().await {
            if diesel::sql_query("SELECT 1").execute(&mut *conn).await.is_ok() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("Postgres did not come back");
}

/// Cut every client connection to the database but this one; Debezium's
/// replication connection stays.
async fn cut_clients(rig: &Rig) {
    let mut conn = rig.pg.diesel.get().await.unwrap();
    diesel::sql_query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
          WHERE backend_type = 'client backend' AND datname = current_database() AND pid <> pg_backend_pid()",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
}

#[tokio::test]
async fn every_change_reaches_history_once_through_each_fault() {
    if !cdc_enabled() || std::env::var("AGENTD_FAULT_TESTS").as_deref() != Ok("1") {
        return;
    }
    let Some(rig) = Rig::new().await else { return };
    let postgres = Container::named("AGENTD_POSTGRES_CONTAINER", "postgres");
    let debezium = Container::named("AGENTD_DEBEZIUM_CONTAINER", "debezium");
    let broker = Container::named("AGENTD_FEED_CONTAINER", "kafka");
    let stalled = Arc::new(Stalled::default());
    let mut tasks = kit::spawn(CONSUMER, stalling(&stalled).await);

    let (st, vault, created) = call(&rig, "POST", "/v1/vaults", Some(json!({"display_name": "f"}))).await;
    assert_eq!(st, StatusCode::OK, "{vault}");
    let v = vault["id"].as_str().unwrap().to_owned();
    let mut expected = vec![("insert".to_owned(), "apikey_01a".to_owned(), created)];
    kept_once(&rig, &v, &expected, "no fault").await;

    // Postgres's slot to Debezium Server: Debezium stops, or crashes without flushing its offsets
    // and the platform starts it again. The slot keeps what it has not read, and what it sent since
    // its last flush is sent again.
    let fault = debezium.broken("stop", "start");
    expected.extend(updates(&rig, &v, MISSED).await);
    drop(fault);
    kept_once(&rig, &v, &expected, "Debezium stopped").await;
    let fault = debezium.broken("kill", "start");
    expected.extend(updates(&rig, &v, MISSED).await);
    drop(fault);
    kept_once(&rig, &v, &expected, "Debezium crashed").await;

    // Debezium Server to the broker: the broker is down briefly, so Debezium holds what it read; down
    // long enough that Debezium's sink fails and it restarts its connector (or exits, and the
    // platform starts it), reading again from the slot; then hung.
    let fault = broker.broken("stop", "start");
    expected.extend(updates(&rig, &v, MISSED).await);
    tokio::time::sleep(OUTAGE).await;
    drop(fault);
    kept_once(&rig, &v, &expected, "a short broker outage").await;
    let since = Timestamp::now().as_datetime().timestamp();
    let fault = broker.broken("stop", "start");
    expected.extend(updates(&rig, &v, MISSED).await);
    sink_failed(&debezium, since).await;
    drop(fault);
    kept_once(&rig, &v, &expected, "a long broker outage").await;
    let fault = broker.broken("pause", "unpause");
    expected.extend(updates(&rig, &v, MISSED).await);
    tokio::time::sleep(OUTAGE).await;
    drop(fault);
    kept_once(&rig, &v, &expected, "a hung broker").await;

    // The broker to the recorders: agentd stops mid-batch, after a recorder stored a change and
    // before the feed learned it, and starts again; what it had not acknowledged is delivered again.
    let batch = expected.len();
    stalled.armed.store(true, Ordering::SeqCst);
    expected.extend(updates(&rig, &v, 20).await);
    let stored = tokio::time::timeout(Duration::from_secs(120), stalled.stored.notified()).await;
    assert!(stored.is_ok(), "no recorder stored a vault change and stalled");
    assert!(kit::kept(&rig, "vaults", "vault_id", &v).await.len() > batch, "the stalled change is stored");
    stop(tasks).await;
    tasks = kit::spawn(CONSUMER, stalling(&stalled).await);
    let again = tokio::time::timeout(Duration::from_secs(120), stalled.again.notified()).await;
    assert!(again.is_ok(), "the feed did not deliver the stalled change again");
    kept_once(&rig, &v, &expected, "an agentd restart").await;

    // The recorders to Postgres: their connections are cut mid-batch, leaving Debezium's slot, then
    // Postgres hangs.
    expected.extend(updates(&rig, &v, MISSED).await);
    cut_clients(&rig).await;
    kept_once(&rig, &v, &expected, "cut connections").await;
    expected.extend(updates(&rig, &v, MISSED).await);
    let fault = postgres.broken("pause", "unpause");
    tokio::time::sleep(OUTAGE).await;
    drop(fault);
    kept_once(&rig, &v, &expected, "a hung Postgres").await;

    // Every boundary at once: Postgres restarts, cutting Debezium's slot, the recorders' pool and
    // the rig's.
    expected.extend(updates(&rig, &v, MISSED).await);
    postgres.docker("restart");
    postgres_back(&rig).await;
    expected.extend(updates(&rig, &v, MISSED).await);
    kept_once(&rig, &v, &expected, "a Postgres restart").await;
    let slot = store_postgres::read_slot(&rig.pg.diesel, "agentd_changes").await.unwrap();
    assert!(!slot.lost, "the slot survives a restart: {slot:?}");

    stop(tasks).await;
}
