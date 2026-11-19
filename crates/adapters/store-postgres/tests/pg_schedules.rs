//! Schedules over live Postgres: the tick writes each due schedule's latest
//! occurrence to the fire queue in the transaction that moves it on, the
//! workers run it once, and the outbox's changes reach the sink in order.
//! Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, DurationRound, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Bool, Nullable, Text, Timestamptz};
use diesel_async::RunQueryDsl;
use shared::AuthContext;
use shared::ports::changes::{Change, ChangeHandler};
use shared::ports::fire::{FireError, FireHandler, FireRegistry, FireWorker};
use shared::ports::{FakeScheduler, FireTarget, ScheduleChange, ScheduleSpec, SchedulerPort, TenantRef};
use store_postgres::{PgBackend, PgFireClaims, PgScheduler, ScheduleOutbox};

/// A tick claims every due schedule in the database and a worker every due
/// fire, so the tests that tick or run fires take turns.
static TICKS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

/// Records each fire it is handed; a target in `failing` fails that many times first.
#[derive(Default)]
struct Seen {
    fires: Mutex<Vec<(String, DateTime<Utc>)>>,
    failing: Mutex<BTreeMap<String, usize>>,
}

impl Seen {
    fn of(&self, target: &FireTarget) -> Vec<DateTime<Utc>> {
        self.fires.lock().unwrap().iter().filter(|(id, _)| *id == target.id).map(|(_, at)| *at).collect()
    }
}

#[async_trait]
impl FireHandler for Seen {
    fn kind(&self) -> &'static str {
        "schedtest"
    }

    async fn fire(&self, _: &AuthContext, id: &str, at: DateTime<Utc>) -> Result<(), FireError> {
        self.fires.lock().unwrap().push((id.to_owned(), at));
        let mut failing = self.failing.lock().unwrap();
        match failing.get_mut(id) {
            Some(left) if *left > 0 => {
                *left -= 1;
                Err(FireError::Failed("down".into()))
            }
            _ => Ok(()),
        }
    }
}

fn spec(id: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: "0 * * * *".into(),
        timezone: "UTC".into(),
        target: FireTarget::new("schedtest", id),
        tenant: TenantRef {
            org_id: "org_s".into(),
            workspace_id: "wrkspc_s".into(),
            api_key_id: "apikey_s".into(),
        },
    }
}

fn uniq(tag: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    format!("{tag}{}{}", Utc::now().timestamp_micros(), NEXT.fetch_add(1, Ordering::Relaxed))
}

async fn exec(pg: &PgBackend, statement: &str, target: &FireTarget) {
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query(statement).bind::<Text, _>(target.key()).execute(&mut conn).await.unwrap();
}

/// Make `target` due at its occurrence `hours` before this hour's.
async fn due(pg: &PgBackend, target: &FireTarget, hours: i64) {
    let statement = format!(
        "UPDATE schedules SET next_fire_at = date_trunc('hour', now()) - interval '{hours} hours' WHERE target = $1"
    );
    exec(pg, &statement, target).await;
}

fn this_hour() -> DateTime<Utc> {
    Utc::now().duration_trunc(Duration::hours(1)).unwrap()
}

/// Let `target`'s queued fires run now, whatever their jitter.
async fn run_now(pg: &PgBackend, target: &FireTarget) {
    exec(
        pg,
        "UPDATE fire_claims SET due_at = now() - interval '1 second' WHERE target = $1 AND NOT done",
        target,
    )
    .await;
}

#[derive(QueryableByName, Debug)]
struct Queued {
    #[diesel(sql_type = Timestamptz)]
    scheduled_at: DateTime<Utc>,
    #[diesel(sql_type = Bool)]
    done: bool,
    #[diesel(sql_type = Nullable<Text>)]
    outcome: Option<String>,
}

async fn queued(pg: &PgBackend, target: &FireTarget) -> Vec<Queued> {
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query(
        "SELECT scheduled_at, done, outcome FROM fire_claims WHERE target = $1 ORDER BY scheduled_at",
    )
    .bind::<Text, _>(target.key())
    .load(&mut conn)
    .await
    .unwrap()
}

struct Rig {
    pg: PgBackend,
    seen: Arc<Seen>,
    fires: Arc<FireRegistry>,
    sched: PgScheduler,
}

impl Rig {
    async fn new() -> Self {
        let pg = backend().await;
        let seen = Arc::new(Seen::default());
        let fires = Arc::new(FireRegistry::new(Arc::new(PgFireClaims::new(pg.clone()))));
        fires.register(seen.clone()).unwrap();
        Self { sched: PgScheduler::new(pg.clone()), pg, seen, fires }
    }

    /// Run every due fire, as one replica's worker does.
    async fn work(&self) {
        let worker = FireWorker::new(self.fires.clone(), |_, _| {});
        while worker.run_due().await.unwrap() > 0 {}
    }

    /// An hourly schedule due at its occurrence `hours` before this hour's.
    async fn schedule(&self, tag: &str, hours: i64) -> ScheduleSpec {
        let s = spec(&uniq(tag));
        self.sched.upsert(s.clone()).await.unwrap();
        due(&self.pg, &s.target, hours).await;
        s
    }
}

#[tokio::test]
async fn a_due_occurrence_fires_once_across_two_schedulers_and_moves_to_its_next_time() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let other = PgScheduler::new(rig.pg.clone());
    let s = rig.schedule("twice", 0).await;
    let (ra, rb) = tokio::join!(rig.sched.tick(), other.tick());
    ra.unwrap();
    rb.unwrap();
    assert_eq!(queued(&rig.pg, &s.target).await.len(), 1, "one occurrence in the queue");
    rig.sched.tick().await.unwrap();
    assert_eq!(queued(&rig.pg, &s.target).await.len(), 1, "moved on to its next time");
    run_now(&rig.pg, &s.target).await;
    rig.work().await;
    rig.work().await;
    assert_eq!(rig.seen.of(&s.target).len(), 1, "one fire for one occurrence");
    assert_eq!(queued(&rig.pg, &s.target).await[0].outcome.as_deref(), Some("ran"));
    rig.sched.delete(&s.target).await.unwrap();
}

#[tokio::test]
async fn two_hundred_fifty_due_schedules_all_fire_in_one_tick_behind_older_ones() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let mut older = Vec::new();
    for _ in 0..150 {
        older.push(rig.schedule("older", 2).await);
    }
    let mut fresh = Vec::new();
    for _ in 0..250 {
        fresh.push(rig.schedule("burst", 0).await);
    }
    let written = rig.sched.tick().await.unwrap();
    assert!(written >= 400, "{written}");
    for s in older.iter().chain(&fresh) {
        assert_eq!(queued(&rig.pg, &s.target).await.len(), 1, "{}", s.target.key());
    }
    for s in older.iter().chain(&fresh) {
        rig.sched.delete(&s.target).await.unwrap();
    }
}

#[tokio::test]
async fn a_fire_that_fails_is_run_again_then_done() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let s = rig.schedule("flaky", 0).await;
    rig.seen.failing.lock().unwrap().insert(s.target.id.clone(), 1);
    rig.sched.tick().await.unwrap();
    run_now(&rig.pg, &s.target).await;
    rig.work().await;
    let first = queued(&rig.pg, &s.target).await;
    assert_eq!((first.len(), first[0].done), (1, false), "it waits for its backoff");
    run_now(&rig.pg, &s.target).await;
    rig.work().await;
    assert_eq!(rig.seen.of(&s.target).len(), 2);
    assert_eq!(queued(&rig.pg, &s.target).await[0].outcome.as_deref(), Some("ran"));
    rig.sched.delete(&s.target).await.unwrap();
}

#[tokio::test]
async fn a_crash_after_the_tick_commits_loses_nothing() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let ticked = Rig::new().await;
    let s = ticked.schedule("crash", 0).await;
    ticked.sched.tick().await.unwrap();
    drop(ticked);
    let restarted = Rig::new().await;
    run_now(&restarted.pg, &s.target).await;
    restarted.work().await;
    assert_eq!(restarted.seen.of(&s.target).len(), 1, "a worker on a new replica runs it");
    restarted.sched.delete(&s.target).await.unwrap();
}

#[tokio::test]
async fn after_downtime_only_the_latest_missed_occurrence_fires() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let s = rig.schedule("downtime", 3).await;
    rig.sched.tick().await.unwrap();
    let rows = queued(&rig.pg, &s.target).await;
    assert_eq!(rows.len(), 1, "no backfill: {rows:?}");
    assert_eq!(rows[0].scheduled_at, this_hour(), "the latest missed occurrence");
    rig.sched.delete(&s.target).await.unwrap();
}

#[tokio::test]
async fn a_fire_not_run_before_its_following_occurrence_is_skipped() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let s = ScheduleSpec { cron: "* * * * *".into(), ..spec(&uniq("skipped")) };
    let missed = Utc::now().duration_trunc(Duration::minutes(1)).unwrap() - Duration::minutes(5);
    rig.fires.enqueue(&s, missed).await.unwrap();
    run_now(&rig.pg, &s.target).await;
    rig.work().await;
    assert!(rig.seen.of(&s.target).is_empty());
    assert_eq!(queued(&rig.pg, &s.target).await[0].outcome.as_deref(), Some("skipped"));
}

#[tokio::test]
async fn an_unpaused_schedule_does_not_fire_the_time_it_missed_and_a_paused_one_waits() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let (unpaused, paused) = (rig.schedule("unpaused", 0).await, rig.schedule("paused", 0).await);
    for s in [&unpaused, &paused] {
        rig.sched.pause(&s.target).await.unwrap();
        due(&rig.pg, &s.target, 1).await;
    }
    rig.sched.unpause(&unpaused.target).await.unwrap();
    rig.sched.tick().await.unwrap();
    assert!(queued(&rig.pg, &unpaused.target).await.is_empty(), "no backfill on unpause");
    assert!(queued(&rig.pg, &paused.target).await.is_empty(), "paused");
    assert!(!rig.sched.describe(&unpaused.target).await.unwrap().paused);
    assert!(rig.sched.describe(&paused.target).await.unwrap().paused);
    for s in [&unpaused, &paused] {
        rig.sched.delete(&s.target).await.unwrap();
    }
}

#[tokio::test]
async fn an_unreadable_schedule_is_paused_and_does_not_hold_the_batch() {
    if !enabled() {
        return;
    }
    let _turn = TICKS.lock().await;
    let rig = Rig::new().await;
    let (bad, good) = (rig.schedule("unreadable", 0).await, rig.schedule("readable", 0).await);
    exec(&rig.pg, "UPDATE schedules SET spec = '{\"cron\": 7}' WHERE target = $1", &bad.target).await;
    rig.sched.tick().await.unwrap();
    assert!(rig.sched.describe(&bad.target).await.unwrap().paused, "paused");
    assert!(queued(&rig.pg, &bad.target).await.is_empty());
    assert_eq!(queued(&rig.pg, &good.target).await.len(), 1);
    for s in [&bad, &good] {
        rig.sched.delete(&s.target).await.unwrap();
    }
}

#[tokio::test]
async fn the_outbox_s_changes_reach_the_sink_in_order_and_its_deletes_are_not_changes() {
    let s = spec(&uniq("order"));
    let sink = Arc::new(FakeScheduler::new());
    let outbox = ScheduleOutbox::new(sink.clone());
    let event = |op: &str, change: &ScheduleChange| {
        let row = serde_json::json!({"seq": 1, "target": s.target.key(), "change": serde_json::to_string(change).unwrap()});
        let (before, after) = match op {
            "d" => (row, serde_json::Value::Null),
            _ => (serde_json::Value::Null, row),
        };
        Change::from_debezium(&serde_json::json!({"op": op, "before": before, "after": after, "source": {"table": "schedule_outbox"}})).unwrap()
    };
    let changes = [
        ScheduleChange::Upsert { spec: s.clone() },
        ScheduleChange::Pause { target: s.target.clone() },
        ScheduleChange::Delete { target: s.target.clone() },
        ScheduleChange::Upsert { spec: s.clone() },
    ];
    for change in &changes {
        outbox.handle(&event("c", change)).await.unwrap();
        outbox.handle(&event("d", change)).await.unwrap();
    }
    assert!(!sink.describe(&s.target).await.unwrap().paused, "the last change, a fresh upsert, wins");
    assert_eq!(sink.registered().iter().filter(|r| r.target == s.target).count(), 1);
}

#[tokio::test]
async fn an_outbox_change_that_never_decodes_is_passed_over() {
    let s = spec(&uniq("poison"));
    let sink = Arc::new(FakeScheduler::new());
    let outbox = ScheduleOutbox::new(sink.clone());
    let insert = |change: serde_json::Value| {
        let row = serde_json::json!({"seq": 1, "target": s.target.key(), "change": change.to_string()});
        Change::from_debezium(&serde_json::json!({"op": "c", "before": null, "after": row, "source": {"table": "schedule_outbox"}})).unwrap()
    };
    outbox.handle(&insert(serde_json::json!({}))).await.unwrap();
    let upsert = serde_json::to_value(ScheduleChange::Upsert { spec: s.clone() }).unwrap();
    outbox.handle(&insert(upsert)).await.unwrap();
    assert!(!sink.describe(&s.target).await.unwrap().paused, "the change after it still applies");
}
