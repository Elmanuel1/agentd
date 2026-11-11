//! The session event log over live Postgres: ordered appends, stamps, the
//! stream's tail read, the session delete, retention, and the overload gate.
//! Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use sessions::events::{Event, EventPayload, SessionEvent, StopReason, UserEvent};
use sessions::retention::{ArchivedCursor, ArchivedSessions, Retention, RunLock};
use sessions::store::{DatabaseLoad, SessionEventLog, SessionEventReads, SessionsStore, StoreError, Tail};
use sessions::types::{Session, SessionId, SessionStatus};
use shared::{OrgId, Timestamp, WorkspaceId};
use store_postgres::{
    PgBackend, PgLoad, PgRunLock, PgSessionEventLog, PgSessionsStore, PgWorkStore, PoolSettings,
};
use work::store::Reach;
use work::types::{Work, WorkData, WorkDataTag, WorkTag};
use work::{WorkItem, WorkReads, WorkState, WorkStore};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

fn url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into())
}

async fn backend() -> PgBackend {
    PgBackend::connect(&url()).await.unwrap()
}

fn uniq_ws(tag: &str) -> WorkspaceId {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    WorkspaceId::new(format!("wrkspc_{tag}{}{n}", Timestamp::now().as_datetime().timestamp_micros()))
}

/// `Timestamp::now()` as the log stores it, to the microsecond.
fn now() -> Timestamp {
    serde_json::from_value(json!(Timestamp::now())).unwrap()
}

fn ago(seconds: i64) -> Timestamp {
    Timestamp::from_datetime(now().as_datetime() - chrono::Duration::seconds(seconds))
}

/// A fresh idle session, archived at `archived_at` when set.
async fn session(store: &PgSessionsStore, ws: &WorkspaceId, archived_at: Option<Timestamp>) -> Session {
    let mut s: Session =
        serde_json::from_str(include_str!("../../../../conformance/fixtures/session.create.json")).unwrap();
    s.id = SessionId::generate();
    s.archived_at = archived_at;
    store.insert(ws, s.clone()).await.unwrap();
    s
}

fn message() -> Event {
    Event::new(EventPayload::User(UserEvent::Interrupt {}))
}

fn idle() -> Event {
    let mut e = Event::new(SessionEvent::StatusIdle { stop_reason: StopReason::EndTurn {} }.into());
    e.processed_at = Some(ago(5));
    e
}

#[tokio::test]
async fn concurrent_appends_to_one_session_are_read_in_commit_order_each_once() {
    // F-1: a tail reading by position while writers race never skips an
    // event that commits late.
    if !enabled() {
        return;
    }
    let store = PgSessionsStore::new(backend().await);
    let log = Arc::new(PgSessionEventLog::new(backend().await));
    for _ in 0..5 {
        let ws = uniq_ws("order");
        let s = session(&store, &ws, None).await;
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let (log, ws, id) = (log.clone(), ws.clone(), s.id.clone());
                tokio::spawn(async move {
                    let mut sent = Vec::new();
                    for _ in 0..20 {
                        let e = message();
                        log.append_events(&ws, &id, std::slice::from_ref(&e)).await.unwrap();
                        sent.push(e.id);
                    }
                    sent
                })
            })
            .collect();
        let (mut cursor, mut seen) = (0, Vec::new());
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while seen.len() < 160 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the tail skipped an event: {} of 160",
                seen.len()
            );
            let read = log.tail(&ws, &s.id, cursor, &[], 1000).await.unwrap().unwrap();
            for (seq, e) in read.events {
                assert!(seq > cursor, "positions only grow");
                cursor = seq;
                seen.push(e.id);
            }
        }
        let mut sent = HashSet::new();
        for w in writers {
            sent.extend(w.await.unwrap());
        }
        assert_eq!(seen.iter().cloned().collect::<HashSet<_>>(), sent, "every event, none skipped");
        assert_eq!(seen.len(), 160, "each once");
        let logged: Vec<_> = log.events(&ws, &s.id).await.unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(seen, logged, "in the log's order");
    }
}

#[tokio::test]
async fn a_stamp_takes_only_the_unprocessed_events_it_names_once() {
    // F-1: two stamps get disjoint events; an unread event and a status
    // event keep what they had.
    if !enabled() {
        return;
    }
    let store = PgSessionsStore::new(backend().await);
    let log = PgSessionEventLog::new(backend().await);
    let ws = uniq_ws("stamp");
    let s = session(&store, &ws, None).await;
    let (a, b, unread, status) = (message(), message(), message(), idle());
    log.append_events(&ws, &s.id, &[a.clone(), b.clone(), status.clone(), unread.clone()]).await.unwrap();
    let named = [a.id.clone(), b.id.clone(), status.id.clone()];
    let (t1, t2) = (now(), ago(-1));
    let (first, second) = tokio::join!(
        log.update_processed_at(&ws, &s.id, &named, t1),
        log.update_processed_at(&ws, &s.id, &named, t2),
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    let ids = |events: &[Event]| events.iter().map(|e| e.id.clone()).collect::<HashSet<_>>();
    assert!(ids(&first).is_disjoint(&ids(&second)), "{first:?} {second:?}");
    assert_eq!(
        ids(&first).union(&ids(&second)).cloned().collect::<HashSet<_>>(),
        HashSet::from([a.id, b.id])
    );
    let logged = log.events(&ws, &s.id).await.unwrap();
    assert_eq!(logged[2], status, "a processed status event is not stamped again");
    assert_eq!(logged[3], unread, "an event not named stays queued");
    assert!(logged[..2].iter().all(|e| e.processed_at == Some(t1) || e.processed_at == Some(t2)));
}

#[tokio::test]
async fn the_tail_reads_after_its_cursor_with_its_pending_events_until_the_session_goes() {
    // F-1, F-2, F-3: what a stream's tail reads.
    if !enabled() {
        return;
    }
    let store = PgSessionsStore::new(backend().await);
    let log = PgSessionEventLog::new(backend().await);
    let ws = uniq_ws("tail");
    let s = session(&store, &ws, None).await;
    assert_eq!(log.head(&ws, &s.id).await.unwrap(), 0);
    assert_eq!(
        log.tail(&ws, &s.id, 0, &[], 10).await.unwrap(),
        Some(Tail { archived: false, events: vec![] })
    );
    let (a, b, c) = (message(), message(), message());
    log.append_events(&ws, &s.id, &[a.clone(), b.clone(), c.clone()]).await.unwrap();
    let all = log.tail(&ws, &s.id, 0, &[], 10).await.unwrap().unwrap();
    let seqs: Vec<i64> = all.events.iter().map(|(seq, _)| *seq).collect();
    assert_eq!(
        all.events.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
        vec![a.clone(), b.clone(), c.clone()]
    );
    assert_eq!(log.head(&ws, &s.id).await.unwrap(), seqs[2]);
    // After b, with a pending: a and c. The limit counts only events after the cursor.
    let read = log.tail(&ws, &s.id, seqs[1], &[seqs[0]], 1).await.unwrap().unwrap();
    assert_eq!(read, Tail { archived: false, events: vec![(seqs[0], a), (seqs[2], c)] });
    let read = log.tail(&ws, &s.id, 0, &[], 2).await.unwrap().unwrap();
    assert_eq!(read.events.len(), 2, "a full page");

    let mut archived = s.clone();
    archived.archived_at = Some(now());
    store.replace(&ws, &s, archived.clone()).await.unwrap();
    assert_eq!(
        log.tail(&ws, &s.id, seqs[2], &[], 10).await.unwrap(),
        Some(Tail { archived: true, events: vec![] })
    );
    store.delete(&ws, &s.id).await.unwrap();
    assert_eq!(log.tail(&ws, &s.id, 0, &[], 10).await.unwrap(), None, "a deleted session reads as gone");
}

fn queued(ws: &WorkspaceId, session: &SessionId) -> WorkItem {
    let created = now();
    WorkItem {
        workspace: ws.clone(),
        org: OrgId::new("org_w"),
        queue: "hosted".into(),
        work: Work {
            id: session.to_string(),
            acknowledged_at: None,
            actor: None,
            created_at: created,
            data: WorkData { id: session.to_string(), tag: WorkDataTag::Session },
            environment_id: "env_log".into(),
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

#[tokio::test]
async fn a_delete_refuses_a_running_session_and_stops_the_work_item_of_one_it_deletes() {
    // E-4: one transaction under the append lock.
    if !enabled() {
        return;
    }
    let store = PgSessionsStore::new(backend().await);
    let work = PgWorkStore::new(backend().await);
    let ws = uniq_ws("delete");
    let s = session(&store, &ws, None).await;
    work.queue(&queued(&ws, &s.id)).await.unwrap();

    let mut running = s.clone();
    running.status = SessionStatus::Running;
    store.replace(&ws, &s, running.clone()).await.unwrap();
    assert!(matches!(store.delete(&ws, &s.id).await, Err(StoreError::Conflict)));
    let item = work.get(&ws, s.id.as_str(), &Reach::default()).await.unwrap().unwrap();
    assert_eq!(item.work.state, WorkState::Queued, "a refused delete stops nothing");
    assert_eq!(store.get(&ws, &s.id).await.unwrap(), running);

    store.replace(&ws, &running, s.clone()).await.unwrap();
    assert_eq!(store.delete(&ws, &s.id).await.unwrap(), s);
    let item = work.get(&ws, s.id.as_str(), &Reach::default()).await.unwrap().unwrap();
    assert_eq!(item.work.state, WorkState::Stopped);
    assert!(matches!(store.delete(&ws, &s.id).await, Err(StoreError::NotFound)));
}

#[tokio::test]
async fn deleting_logs_removes_each_named_session_s_events_and_no_other() {
    // E-4.
    if !enabled() {
        return;
    }
    let store = PgSessionsStore::new(backend().await);
    let log = PgSessionEventLog::new(backend().await);
    let ws = uniq_ws("purge");
    let (one, two, kept) = (
        session(&store, &ws, None).await,
        session(&store, &ws, None).await,
        session(&store, &ws, None).await,
    );
    let e = message();
    for s in [&one, &two, &kept] {
        log.append_events(&ws, &s.id, &[message()]).await.unwrap();
    }
    log.append_events(&ws, &kept.id, std::slice::from_ref(&e)).await.unwrap();
    log.delete_events_of(&[(ws.clone(), one.id.clone()), (ws.clone(), two.id.clone())]).await.unwrap();
    assert_eq!(log.events(&ws, &one.id).await.unwrap(), vec![]);
    assert_eq!(log.events(&ws, &two.id).await.unwrap(), vec![]);
    assert_eq!(log.events(&ws, &kept.id).await.unwrap().len(), 2);
}

const DAY: i64 = 24 * 60 * 60;

/// Retention runs share one lock name, so the tests that run them take turns.
static RETENTION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn retention(pg: &PgBackend, days: u64) -> Retention {
    Retention::new(
        Box::new(PgSessionsStore::new(pg.clone())),
        Box::new(PgSessionEventLog::new(pg.clone())),
        Box::new(PgRunLock::new(pg.clone())),
        Duration::from_secs(days * 24 * 60 * 60),
    )
}

/// A session archived at `at`, holding one event.
async fn archived(pg: &PgBackend, ws: &WorkspaceId, at: Timestamp) -> SessionId {
    let store = PgSessionsStore::new(pg.clone());
    let s = session(&store, ws, Some(at)).await;
    PgSessionEventLog::new(pg.clone()).append_events(ws, &s.id, &[message()]).await.unwrap();
    s.id
}

async fn events(pg: &PgBackend, ws: &WorkspaceId, id: &SessionId) -> usize {
    PgSessionEventLog::new(pg.clone()).events(ws, id).await.unwrap().len()
}

#[tokio::test]
async fn retention_deletes_the_events_of_sessions_archived_longer_ago_than_its_period() {
    // E-2: with 1 day, two days ago goes and today stays; the first run walks
    // from the oldest, a later one an hour back from the cutoff.
    if !enabled() {
        return;
    }
    let _turn = RETENTION.lock().await;
    let pg = backend().await;
    let ws = uniq_ws("retain");
    let old = archived(&pg, &ws, ago(2 * DAY)).await;
    let month = archived(&pg, &ws, ago(30 * DAY)).await;
    let today = archived(&pg, &ws, ago(60)).await;
    let missed = archived(&pg, &ws, ago(DAY + 40 * 60)).await;
    let r = retention(&pg, 1);

    assert!(r.run(false, Timestamp::now()).await.unwrap());
    assert_eq!(events(&pg, &ws, &missed).await, 0, "a run missed for 40 minutes is caught by the next");
    assert_eq!(events(&pg, &ws, &old).await, 1, "a later run walks only the last hour before the cutoff");
    assert_eq!(events(&pg, &ws, &month).await, 1);

    assert!(r.run(true, Timestamp::now()).await.unwrap());
    assert_eq!(events(&pg, &ws, &old).await, 0);
    assert_eq!(events(&pg, &ws, &month).await, 0, "the first run walks from the oldest");
    assert_eq!(events(&pg, &ws, &today).await, 1, "archived inside the period keeps its events");
}

#[tokio::test]
async fn the_archived_walk_pages_in_archive_order_without_repeating() {
    // E-2: the keyset on (archived_at, session_id).
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let store = PgSessionsStore::new(pg.clone());
    let ws = uniq_ws("walk");
    let at = ago(400 * DAY);
    let mut ours = Vec::new();
    for _ in 0..3 {
        ours.push(archived(&pg, &ws, at).await);
    }
    ours.push(
        archived(&pg, &ws, Timestamp::from_datetime(at.as_datetime() + chrono::Duration::seconds(1))).await,
    );
    let to = Timestamp::from_datetime(at.as_datetime() + chrono::Duration::seconds(2));
    let from = ArchivedCursor { archived_at: ago(401 * DAY).to_wire(), session_id: String::new() };
    let (mut after, mut walked) = (from, Vec::new());
    loop {
        let page = store.archived_between(&after, to, 2).await.unwrap();
        let Some(last) = page.last() else { break };
        after =
            ArchivedCursor { archived_at: last.archived_at.clone(), session_id: last.session.to_string() };
        walked.extend(page.into_iter().filter(|s| s.workspace == ws).map(|s| s.session));
    }
    let mut first_three = ours[..3].to_vec();
    first_three.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    assert_eq!(walked[..3], first_three[..], "within one archived_at, by session id");
    assert_eq!(walked[3], ours[3]);
    assert_eq!(walked.len(), 4);
}

#[tokio::test]
async fn one_holder_at_a_time_and_a_failed_or_panicked_or_dropped_run_frees_the_lock() {
    // E-2: the run lock across replicas.
    if !enabled() {
        return;
    }
    let (a, b) = (PgRunLock::new(backend().await), PgRunLock::new(backend().await));
    let name = format!("test_lock_{}", Timestamp::now().as_datetime().timestamp_micros());
    let (held_tx, held_rx) = tokio::sync::oneshot::channel::<()>();
    let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
    let holder = {
        let name = name.clone();
        tokio::spawn(async move {
            a.exclusive(
                &name,
                Box::pin(async move {
                    held_tx.send(()).unwrap();
                    go_rx.await.unwrap();
                    Ok(())
                }),
            )
            .await
        })
    };
    held_rx.await.unwrap();
    let ran =
        b.exclusive(&name, Box::pin(async { panic!("must not run while another holds the lock") })).await;
    assert!(!ran.unwrap(), "a second holder is refused");
    go_tx.send(()).unwrap();
    assert!(holder.await.unwrap().unwrap());

    let failed = b.exclusive(&name, Box::pin(async { Err(StoreError::NotFound) })).await;
    assert!(matches!(failed, Err(StoreError::NotFound)));
    assert!(b.exclusive(&name, Box::pin(async { Ok(()) })).await.unwrap(), "free after a failed run");

    let panicked = {
        let (b, name) = (PgRunLock::new(backend().await), name.clone());
        tokio::spawn(async move { b.exclusive(&name, Box::pin(async { panic!("mid-page") })).await })
    };
    assert!(panicked.await.is_err());
    assert!(b.exclusive(&name, Box::pin(async { Ok(()) })).await.unwrap(), "free after a panicked run");

    let dropped = b.exclusive(&name, Box::pin(std::future::pending()));
    assert!(tokio::time::timeout(Duration::from_millis(300), dropped).await.is_err());
    assert!(b.exclusive(&name, Box::pin(async { Ok(()) })).await.unwrap(), "free after a cancelled run");
}

#[tokio::test]
async fn a_spawned_retention_reports_each_run_it_completes() {
    // E-2: the run behind `agentd.retention.last_success`.
    if !enabled() {
        return;
    }
    let _turn = RETENTION.lock().await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = retention(&backend().await, 3650).spawn(move |at| tx.send(at).unwrap());
    let before = Timestamp::now();
    let at = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.unwrap().unwrap();
    assert!(at >= before);
    task.abort();
}

#[tokio::test]
async fn a_checkout_that_waits_over_a_second_trips_the_gate() {
    // F-5.
    if !enabled() {
        return;
    }
    let settings = PoolSettings { max_size: 1, wait: Duration::from_secs(5) };
    let pg = PgBackend::connect_with(&url(), settings).await.unwrap();
    let load = PgLoad::new(pg.diesel.clone());
    load.measure().await.unwrap();
    assert!(!load.overloaded(), "a quiet pool and a database well under max_connections");
    let held = pg.diesel.get().await.unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1300)).await;
        drop(held);
    });
    drop(pg.diesel.get().await.unwrap());
    release.await.unwrap();
    assert!(load.overloaded(), "a checkout waited over 1 s within the last 10 s");
}
