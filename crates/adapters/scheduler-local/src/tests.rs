//! LocalScheduler fire-contract tests.

use super::*;
use parking_lot::Mutex as PMutex;
use shared::ports::fire::{FireError, FireHandler};
use shared::ports::TenantRef;
use shared::AuthContext;

/// Records deployment fires; can be armed to fail.
#[derive(Default)]
struct RecordingSink {
    fires: PMutex<Vec<(String, DateTime<Utc>)>>,
    tenants: PMutex<Vec<AuthContext>>,
    fail: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl FireHandler for RecordingSink {
    fn kind(&self) -> &'static str {
        "deployment"
    }
    async fn fire(
        &self,
        tenant: &AuthContext,
        id: &str,
        scheduled_at: DateTime<Utc>,
    ) -> Result<(), FireError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(FireError::Failed("agentd down".into()));
        }
        self.fires.lock().push((format!("deployment/{id}"), scheduled_at));
        self.tenants.lock().push(tenant.clone());
        Ok(())
    }
}

fn scheduler(sink: Arc<RecordingSink>) -> LocalScheduler {
    let fires = Arc::new(FireRegistry::new());
    fires.register(sink).unwrap();
    LocalScheduler::new(fires)
}

fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

fn spec(cron: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: cron.into(),
        timezone: "UTC".into(),
        target: FireTarget::new("deployment", "depl_01test"),
        tenant: TenantRef {
            org_id: "org_t".into(),
            workspace_id: "wrkspc_01t".into(),
            api_key_id: "apikey_01t".into(),
        },
    }
}

async fn rig(cron: &str) -> (Arc<LocalScheduler>, Arc<RecordingSink>) {
    let sink = Arc::new(RecordingSink::default());
    let sched = Arc::new(scheduler(sink.clone()));
    sched.upsert(spec(cron)).await.unwrap();
    // Deterministic baseline: force next_fire from a known instant.
    {
        let mut g = sched.inner.lock();
        for reg in g.values_mut() {
            reg.next_fire = reg.compiled.next_fires(utc("2026-01-01T00:00:30Z"), 1).first().copied();
        }
    }
    (sched, sink)
}

#[tokio::test]
async fn fires_when_due_not_before() {
    let (sched, sink) = rig("*/5 * * * *").await; // next: 00:05
    sched.tick(utc("2026-01-01T00:04:00Z")).await;
    assert!(sink.fires.lock().is_empty(), "not due yet");
    sched.tick(utc("2026-01-01T00:05:00Z")).await;
    let fires = sink.fires.lock();
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].1, utc("2026-01-01T00:05:00Z"));
}

#[tokio::test]
async fn no_backfill_only_latest_missed_occurrence_fires() {
    let (sched, sink) = rig("*/5 * * * *").await; // next: 00:05
                                                  // 27 minutes pass with no ticks (downtime): 00:05/10/15/20/25 missed.
    sched.tick(utc("2026-01-01T00:27:00Z")).await;
    {
        let fires = sink.fires.lock();
        assert_eq!(fires.len(), 1, "exactly one fire, not five");
        assert_eq!(fires[0].1, utc("2026-01-01T00:25:00Z"), "the LATEST missed one");
    }
    // Next fires normally at 00:30.
    sched.tick(utc("2026-01-01T00:30:00Z")).await;
    assert_eq!(sink.fires.lock().len(), 2);
}

#[tokio::test]
async fn transport_failure_drops_tick_no_retry() {
    let (sched, sink) = rig("*/5 * * * *").await;
    sink.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    sched.tick(utc("2026-01-01T00:05:00Z")).await;
    assert!(sink.fires.lock().is_empty(), "fire dropped");
    sink.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    // Immediately re-ticking the same instant does NOT re-fire (dropped
    // means dropped: the schedule already advanced).
    sched.tick(utc("2026-01-01T00:05:30Z")).await;
    assert!(sink.fires.lock().is_empty());
    // The NEXT occurrence fires normally.
    sched.tick(utc("2026-01-01T00:10:00Z")).await;
    assert_eq!(sink.fires.lock().len(), 1);
}

#[tokio::test]
async fn pause_suppresses_unpause_resumes_without_backfill() {
    let (sched, sink) = rig("*/5 * * * *").await;
    let target = FireTarget::new("deployment", "depl_01test");
    sched.pause(&target).await.unwrap();
    sched.tick(utc("2026-01-01T00:05:00Z")).await;
    sched.tick(utc("2026-01-01T00:10:00Z")).await;
    assert!(sink.fires.lock().is_empty(), "paused = no fires");
    sched.unpause(&target).await.unwrap();
    assert!(!sched.describe(&target).await.unwrap().paused);
    // Missed occurrences during pause are NOT backfilled.
    // (next_fire re-computed from real now; assert no immediate fire for
    // the past occurrences)
    sched.tick(utc("2026-01-01T00:10:30Z")).await;
    assert!(sink.fires.lock().is_empty());
}

#[tokio::test]
async fn delete_and_describe() {
    let (sched, _sink) = rig("0 0 * * *").await;
    let target = FireTarget::new("deployment", "depl_01test");
    let st = sched.describe(&target).await.unwrap();
    assert!(!st.paused);
    assert!(st.last_fire.is_none());
    sched.delete(&target).await.unwrap();
    assert!(matches!(sched.describe(&target).await, Err(SchedulerError::Unknown(_))));
}

#[tokio::test]
async fn upsert_rejects_bad_cron_via_shared_engine() {
    let sched = scheduler(Arc::new(RecordingSink::default()));
    let mut s = spec("99 * * * *");
    assert!(matches!(sched.upsert(s.clone()).await, Err(SchedulerError::InvalidSpec(_))));
    s.cron = "0 0 * * *".into();
    s.timezone = "Mars/Olympus".into();
    assert!(matches!(sched.upsert(s).await, Err(SchedulerError::InvalidSpec(_))));
}

#[tokio::test]
async fn last_fire_is_recorded() {
    let (sched, _sink) = rig("*/5 * * * *").await;
    sched.tick(utc("2026-01-01T00:05:00Z")).await;
    let target = FireTarget::new("deployment", "depl_01test");
    assert_eq!(sched.describe(&target).await.unwrap().last_fire, Some(utc("2026-01-01T00:05:00Z")));
}

#[tokio::test]
async fn fire_runs_under_the_schedules_tenant_with_no_api_key_registered() {
    let (sched, sink) = rig("*/5 * * * *").await;
    sched.tick(utc("2026-01-01T00:05:00Z")).await;
    let tenants = sink.tenants.lock();
    let tenant = tenants.first().expect("fired");
    assert_eq!(tenant.org.as_str(), "org_t");
    assert_eq!(tenant.workspace.as_str(), "wrkspc_01t");
    assert_eq!(tenant.api_key_id, "apikey_01t");
}
