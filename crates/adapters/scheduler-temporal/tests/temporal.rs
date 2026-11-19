//! The Temporal sink against the Temporal at `AGENTD_TEMPORAL_ADDRESS` (a dev
//! server, `temporal server start-dev`), with the fire queue on the Postgres
//! at `DATABASE_URL` when `AGENTD_PG_TESTS=1`; skipped without them.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, DurationRound, Utc};
use scheduler_temporal::{TemporalConfig, TemporalScheduler, spawn_worker};
use shared::AuthContext;
use shared::ports::fire::{FireError, FireHandler, FireRegistry, FireWorker};
use shared::ports::{FireTarget, ScheduleSpec, SchedulerError, SchedulerPort, TenantRef};
use store_postgres::{PgBackend, PgFireClaims};

fn config() -> Option<TemporalConfig> {
    let address = std::env::var("AGENTD_TEMPORAL_ADDRESS").ok()?;
    let task_queue = format!("agentd-test-{}", Utc::now().timestamp_micros());
    Some(TemporalConfig { address, namespace: "default".into(), task_queue })
}

fn spec(cron: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: cron.into(),
        timezone: "UTC".into(),
        target: FireTarget::new("temporaltest", format!("t{}", Utc::now().timestamp_micros())),
        tenant: TenantRef {
            org_id: "org_t".into(),
            workspace_id: "wrkspc_t".into(),
            api_key_id: "apikey_t".into(),
        },
    }
}

#[tokio::test]
async fn pause_and_unpause_reach_the_temporal_schedule() {
    let Some(config) = config() else { return };
    let sink = TemporalScheduler::connect(config).await.unwrap();
    let s = spec("0 3 * * *");
    sink.upsert(s.clone()).await.unwrap();
    assert!(!sink.describe(&s.target).await.unwrap().paused);
    sink.pause(&s.target).await.unwrap();
    assert!(sink.describe(&s.target).await.unwrap().paused);
    sink.unpause(&s.target).await.unwrap();
    assert!(!sink.describe(&s.target).await.unwrap().paused);
    sink.pause(&s.target).await.unwrap();
    sink.upsert(ScheduleSpec { cron: "0 4 * * *".into(), ..s.clone() }).await.unwrap();
    assert!(!sink.describe(&s.target).await.unwrap().paused, "an upsert of an existing schedule replaces it");
    sink.delete(&s.target).await.unwrap();
    assert!(matches!(sink.describe(&s.target).await, Err(SchedulerError::Unknown(_))));
    assert!(matches!(sink.pause(&s.target).await, Err(SchedulerError::Unknown(_))));
    let invalid = ScheduleSpec { cron: "61 * * * *".into(), ..spec("0 3 * * *") };
    assert!(matches!(sink.upsert(invalid).await, Err(SchedulerError::InvalidSpec(_))));
}

/// Records each fire it is handed.
#[derive(Default)]
struct Seen(Mutex<Vec<(String, DateTime<Utc>)>>);

#[async_trait]
impl FireHandler for Seen {
    fn kind(&self) -> &'static str {
        "temporaltest"
    }

    async fn fire(&self, _: &AuthContext, id: &str, at: DateTime<Utc>) -> Result<(), FireError> {
        self.0.lock().unwrap().push((id.to_owned(), at));
        Ok(())
    }
}

#[tokio::test]
async fn a_schedule_on_the_temporal_sink_fires_through_the_queue_once() {
    let Some(config) = config() else { return };
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let url = std::env::var("DATABASE_URL").unwrap();
    let pg = PgBackend::connect(&url).await.unwrap();
    let seen = Arc::new(Seen::default());
    let fires = Arc::new(FireRegistry::new(Arc::new(PgFireClaims::new(pg.clone()))));
    fires.register(seen.clone()).unwrap();
    let sink = TemporalScheduler::connect(config).await.unwrap();
    drop(spawn_worker(&sink, fires.clone()));
    let s = spec("* * * * *");
    sink.upsert(s.clone()).await.unwrap();

    let worker = FireWorker::new(fires.clone(), |_, _| {});
    let mut fired = Vec::new();
    for _ in 0..180 {
        // The occurrence is due after its jitter (5 s to 9 s for a minutely schedule).
        while worker.run_due().await.unwrap() > 0 {}
        fired =
            seen.0.lock().unwrap().iter().filter(|(id, _)| *id == s.target.id).map(|(_, at)| *at).collect();
        if !fired.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    sink.delete(&s.target).await.unwrap();
    assert_eq!(fired.len(), 1, "{fired:?}");
    let at = fired[0];
    assert_eq!(
        at,
        at.duration_trunc(chrono::Duration::minutes(1)).unwrap(),
        "the occurrence's exact listed time"
    );
    while worker.run_due().await.unwrap() > 0 {}
    let again = seen.0.lock().unwrap().iter().filter(|(id, when)| *id == s.target.id && *when == at).count();
    assert_eq!(again, 1, "once");
}
