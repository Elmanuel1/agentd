//! Schedules over live Postgres (K-1, K-2): a due schedule fires once across
//! schedulers, a late one is skipped, and the outbox reaches the sink in
//! order. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use shared::ports::fire::{FireError, FireHandler, FireRegistry};
use shared::ports::{FakeScheduler, FireTarget, ScheduleChange, ScheduleSpec, SchedulerPort, TenantRef};
use shared::AuthContext;
use store_postgres::{PgBackend, PgScheduleRelay, PgScheduler};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

/// Records each fire it is handed.
#[derive(Default)]
struct Seen(Mutex<Vec<(String, DateTime<Utc>)>>);

#[async_trait]
impl FireHandler for Seen {
    fn kind(&self) -> &'static str {
        "schedtest"
    }

    async fn fire(&self, _: &AuthContext, id: &str, at: DateTime<Utc>) -> Result<(), FireError> {
        self.0.lock().unwrap().push((id.to_owned(), at));
        Ok(())
    }
}

fn spec(id: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: "* * * * *".into(),
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
    format!("{tag}{}", Utc::now().timestamp_micros())
}

/// Make `target` due `ago` seconds in the past.
async fn due(pg: &PgBackend, target: &FireTarget, ago: i64) {
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query(format!(
        "UPDATE schedules SET next_fire_at = now() - interval '{ago} seconds' WHERE target = $1"
    ))
    .bind::<Text, _>(target.key())
    .execute(&mut conn)
    .await
    .unwrap();
}

#[tokio::test]
async fn a_due_schedule_fires_once_across_two_schedulers_and_moves_to_its_next_time() {
    if !enabled() {
        return;
    }
    let (pg, seen, fires) = (backend().await, Arc::new(Seen::default()), Arc::new(FireRegistry::new()));
    fires.register(seen.clone()).unwrap();
    let (a, b) = (PgScheduler::new(pg.clone(), fires.clone()), PgScheduler::new(pg.clone(), fires.clone()));
    let s = spec(&uniq("twice"));
    a.upsert(s.clone()).await.unwrap();
    due(&pg, &s.target, 5).await;
    let (ra, rb) = tokio::join!(a.tick(), b.tick());
    ra.unwrap();
    rb.unwrap();
    let fired: Vec<_> = seen.0.lock().unwrap().iter().filter(|(id, _)| *id == s.target.id).cloned().collect();
    assert_eq!(fired.len(), 1, "one fire for one occurrence: {fired:?}");
    a.tick().await.unwrap();
    assert_eq!(seen.0.lock().unwrap().iter().filter(|(id, _)| *id == s.target.id).count(), 1, "moved on");
    a.delete(&s.target).await.unwrap();
}

#[tokio::test]
async fn a_fire_more_than_a_minute_late_is_skipped_and_a_paused_one_waits() {
    if !enabled() {
        return;
    }
    let (pg, seen, fires) = (backend().await, Arc::new(Seen::default()), Arc::new(FireRegistry::new()));
    fires.register(seen.clone()).unwrap();
    let sched = PgScheduler::new(pg.clone(), fires.clone());
    // Hourly, its last time about 30 minutes ago: more than a minute late.
    use chrono::Timelike;
    let minute = (Utc::now().minute() + 30) % 60;
    let late = ScheduleSpec { cron: format!("{minute} * * * *"), ..spec(&uniq("late")) };
    let paused = spec(&uniq("paused"));
    for s in [&late, &paused] {
        sched.upsert(s.clone()).await.unwrap();
    }
    sched.pause(&paused.target).await.unwrap();
    due(&pg, &late.target, 3600).await;
    due(&pg, &paused.target, 5).await;
    sched.tick().await.unwrap();
    let ids: Vec<String> = seen.0.lock().unwrap().iter().map(|(id, _)| id.clone()).collect();
    assert!(!ids.contains(&late.target.id), "no backfill after downtime");
    assert!(!ids.contains(&paused.target.id), "paused");
    assert!(sched.describe(&paused.target).await.unwrap().paused);
    for s in [&late, &paused] {
        sched.delete(&s.target).await.unwrap();
    }
}

#[tokio::test]
async fn the_outbox_reaches_the_sink_in_order() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let s = spec(&uniq("order"));
    let mut conn = pg.diesel.get().await.unwrap();
    for change in [
        ScheduleChange::Upsert { spec: s.clone() },
        ScheduleChange::Pause { target: s.target.clone() },
        ScheduleChange::Delete { target: s.target.clone() },
        ScheduleChange::Upsert { spec: s.clone() },
    ] {
        let doc = serde_json::to_value(&change).unwrap();
        diesel::sql_query("INSERT INTO schedule_outbox (target, change) VALUES ($1, $2)")
            .bind::<Text, _>(s.target.key())
            .bind::<diesel::sql_types::Jsonb, _>(doc)
            .execute(&mut conn)
            .await
            .unwrap();
    }
    let sink = Arc::new(FakeScheduler::new());
    let relay = PgScheduleRelay::new(pg.clone(), sink.clone());
    for _ in 0..20 {
        relay.relay().await.unwrap();
    }
    let state = sink.describe(&s.target).await.unwrap();
    assert!(!state.paused, "the last change, a fresh upsert, wins");
    assert_eq!(sink.registered().iter().filter(|r| r.target == s.target).count(), 1);
}
