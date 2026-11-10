//! Work items over live Postgres: claims, acknowledgements, heartbeats and
//! stops, each one guarded statement. Gated by `AGENTD_PG_TESTS=1` +
//! `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::time::Duration;

use shared::{OrgId, Pagination, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgWorkStore};
use work::store::{Beat, Reach, Scope};
use work::types::{Work, WorkData, WorkDataTag, WorkTag};
use work::{WorkItem, WorkReads, WorkState, WorkStore, RUNTIME};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn store() -> PgWorkStore {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgWorkStore::new(PgBackend::connect(&url).await.unwrap())
}

fn stamp() -> String {
    format!("{}{}", Timestamp::now().as_datetime().timestamp_micros(), rand_suffix())
}

fn rand_suffix() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn at(seconds: i64) -> Timestamp {
    Timestamp::from_datetime(Timestamp::now().as_datetime() + chrono::Duration::seconds(seconds))
}

/// A queued item of `session` in `ws` on `queue`, created `age` seconds ago.
fn queued(ws: &str, session: &str, queue: &str, age: i64) -> WorkItem {
    let created = at(-age);
    WorkItem {
        workspace: WorkspaceId::new(ws),
        org: OrgId::new("org_w"),
        queue: queue.into(),
        work: Work {
            id: session.into(),
            acknowledged_at: None,
            actor: None,
            created_at: created,
            data: WorkData { id: session.into(), tag: WorkDataTag::Session },
            environment_id: format!("env_{queue}"),
            latest_heartbeat_at: None,
            metadata: BTreeMap::new(),
            secret: None,
            started_at: Some(created),
            state: WorkState::Queued,
            stop_requested_at: None,
            stopped_at: None,
            tag: WorkTag::Work,
        },
        last_heartbeat: None,
        lease_until: None,
        handed_out_at: None,
    }
}

fn beat(expected: Option<&str>, ttl: i64) -> Beat {
    let now = Timestamp::now();
    Beat { expected: expected.map(str::to_owned), stamp: now.to_wire(), at: now, lease_until: at(ttl) }
}

#[tokio::test]
async fn two_claims_take_different_items_and_an_unacknowledged_one_comes_back_after_5_s() {
    if !enabled() {
        return;
    }
    let (s, q, ws) = (store().await, format!("env_q{}", stamp()), format!("wrkspc_{}", stamp()));
    s.insert(&queued(&ws, "sesn_old", &q, 20)).await.unwrap();
    s.insert(&queued(&ws, "sesn_new", &q, 10)).await.unwrap();
    let reclaim = Duration::from_secs(5);
    let first = s.claim(&q, reclaim, Timestamp::now()).await.unwrap().unwrap();
    let second = s.claim(&q, reclaim, Timestamp::now()).await.unwrap().unwrap();
    assert_eq!((first.work.id.as_str(), second.work.id.as_str()), ("sesn_old", "sesn_new"), "oldest first");
    assert!(s.claim(&q, reclaim, Timestamp::now()).await.unwrap().is_none(), "both handed out");
    let counts = s.counts(&Scope::Queue(q.clone()), reclaim, Timestamp::now()).await.unwrap();
    assert_eq!((counts.depth, counts.pending), (0, 2));
    let again = s.claim(&q, reclaim, at(6)).await.unwrap().unwrap();
    assert_eq!(again.work.id, "sesn_old", "not acknowledged within 5 s: handed out again");
    let reach = Reach { queue: Some(q.clone()), ..Reach::default() };
    let acked = s.ack(&WorkspaceId::new(&ws), "sesn_old", &reach, Timestamp::now(), at(300)).await.unwrap();
    assert_eq!(acked.unwrap().work.state, WorkState::Starting);
    assert!(s.claim(&q, reclaim, at(60)).await.unwrap().map(|i| i.work.id) != Some("sesn_old".into()));
}

#[tokio::test]
async fn concurrent_claims_never_hand_one_item_out_twice() {
    if !enabled() {
        return;
    }
    let (s, q, ws) = (store().await, format!("env_c{}", stamp()), format!("wrkspc_{}", stamp()));
    for i in 0..8 {
        s.insert(&queued(&ws, &format!("sesn_{i}"), &q, 30 - i)).await.unwrap();
    }
    let claims = (0..8).map(|_| {
        let (s, q) = (s.clone(), q.clone());
        tokio::spawn(async move { s.claim(&q, Duration::from_secs(5), Timestamp::now()).await.unwrap() })
    });
    let mut ids: Vec<String> = Vec::new();
    for c in claims {
        ids.extend(c.await.unwrap().map(|i| i.work.id));
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 8);
}

#[tokio::test]
async fn a_stale_heartbeat_is_refused_and_a_stopping_item_past_its_lease_reads_stopped() {
    if !enabled() {
        return;
    }
    let (s, ws) = (store().await, format!("wrkspc_{}", stamp()));
    let w = WorkspaceId::new(&ws);
    s.insert(&queued(&ws, "sesn_h", "env_h", 1)).await.unwrap();
    let any = Reach::default();
    let (first, extended) =
        s.heartbeat(&w, "sesn_h", &any, &beat(Some("NO_HEARTBEAT"), 2)).await.unwrap().unwrap();
    assert!(extended && first.work.state == WorkState::Active);
    let (_, stale) = s.heartbeat(&w, "sesn_h", &any, &beat(Some("NO_HEARTBEAT"), 2)).await.unwrap().unwrap();
    assert!(!stale, "a heartbeat naming an old lease extends nothing");
    let stopping = s.stop(&w, "sesn_h", &any, false, Timestamp::now()).await.unwrap().unwrap();
    assert_eq!(stopping.work.state, WorkState::Stopping, "a worker holds it");
    let expected = first.last_heartbeat.clone();
    let (after, extended) =
        s.heartbeat(&w, "sesn_h", &any, &beat(expected.as_deref(), 2)).await.unwrap().unwrap();
    assert!(!extended && after.work.state == WorkState::Stopping);
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let read = s.get(&w, "sesn_h").await.unwrap().unwrap();
    assert_eq!(read.work.state, WorkState::Stopped);
    assert!(read.work.stopped_at.is_some());
}

#[tokio::test]
async fn an_active_item_with_no_heartbeats_stays_active_and_a_queued_one_stops_at_once() {
    if !enabled() {
        return;
    }
    let (s, ws) = (store().await, format!("wrkspc_{}", stamp()));
    let w = WorkspaceId::new(&ws);
    let any = Reach::default();
    s.insert(&queued(&ws, "sesn_a", "env_a", 1)).await.unwrap();
    s.heartbeat(&w, "sesn_a", &any, &beat(None, 1)).await.unwrap().unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(s.get(&w, "sesn_a").await.unwrap().unwrap().work.state, WorkState::Active);
    s.insert(&queued(&ws, "sesn_q", "env_a", 1)).await.unwrap();
    let stopped = s.stop(&w, "sesn_q", &any, false, Timestamp::now()).await.unwrap().unwrap();
    assert_eq!(stopped.work.state, WorkState::Stopped, "no worker holds a queued item");
    let lapsed = s.stop(&w, "sesn_a", &any, false, Timestamp::now()).await.unwrap().unwrap();
    assert_eq!(lapsed.work.state, WorkState::Stopped, "its lease ran out: no worker holds it");
}

#[tokio::test]
async fn a_write_misses_an_item_its_path_does_not_reach() {
    if !enabled() {
        return;
    }
    let (s, ws) = (store().await, format!("wrkspc_{}", stamp()));
    let w = WorkspaceId::new(&ws);
    s.insert(&queued(&ws, "sesn_r", RUNTIME, 1)).await.unwrap();
    let elsewhere = Reach { queue: Some("env_other".into()), environment: Some("env_other".into()) };
    assert!(s.ack(&w, "sesn_r", &elsewhere, Timestamp::now(), at(300)).await.unwrap().is_none());
    assert!(s.stop(&w, "sesn_r", &elsewhere, true, Timestamp::now()).await.unwrap().is_none());
    let mut set = BTreeMap::new();
    set.insert("k".to_owned(), "v".to_owned());
    let on_queue = Reach { queue: Some(RUNTIME.into()), ..Reach::default() };
    let updated = s.update_metadata(&w, "sesn_r", &on_queue, &set, &[]).await.unwrap().unwrap();
    assert_eq!(updated.work.metadata, set);
    let removed = s.update_metadata(&w, "sesn_r", &on_queue, &BTreeMap::new(), &["k".into()]).await.unwrap();
    assert!(removed.unwrap().work.metadata.is_empty());
    let found = s.find_on_queue(RUNTIME, "sesn_r").await.unwrap().unwrap();
    assert_eq!(found.workspace, w);
    let owner = s.runtime_owner(&format!("env_{RUNTIME}")).await.unwrap();
    assert!(owner.is_some());
    let listed = s
        .list(
            &Scope::Environment { workspace: w.clone(), environment: format!("env_{RUNTIME}"), queue: None },
            &Pagination { limit: 10, offset: 0 },
        )
        .await
        .unwrap();
    assert_eq!(listed.iter().map(|i| i.work.id.as_str()).collect::<Vec<_>>(), ["sesn_r"]);
    s.stop(&w, "sesn_r", &on_queue, true, Timestamp::now()).await.unwrap();
}
