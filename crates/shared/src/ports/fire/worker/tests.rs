use std::sync::Mutex;

use async_trait::async_trait;
use chrono::DateTime;

use super::*;
use crate::AuthContext;
use crate::ports::fire::{Fire, FireHandler, FireKey, NativeFireClaims};
use crate::ports::{FireTarget, ScheduleSpec, TenantRef};

/// A queue that hands out `due` once, and records each finish and retry;
/// `lost` answers every write as a lease another claim took over.
#[derive(Default)]
struct Queue {
    due: Mutex<Vec<Claimed>>,
    finished: Mutex<Vec<(String, Outcome)>>,
    retried: Mutex<Vec<(String, DateTime<Utc>)>>,
    lost: bool,
}

impl NativeFireClaims for Queue {
    async fn enqueue(&self, _: &[Fire]) -> Result<(), FireError> {
        Ok(())
    }
    async fn claim_due(&self, limit: usize) -> Result<Vec<Claimed>, FireError> {
        let mut due = self.due.lock().unwrap();
        let n = due.len().min(limit);
        Ok(due.drain(..n).collect())
    }
    async fn finish(&self, c: &Claimed, outcome: Outcome) -> Result<bool, FireError> {
        self.finished.lock().unwrap().push((c.key.target.clone(), outcome));
        Ok(!self.lost)
    }
    async fn retry(&self, c: &Claimed, at: DateTime<Utc>) -> Result<bool, FireError> {
        self.retried.lock().unwrap().push((c.key.target.clone(), at));
        Ok(!self.lost)
    }
}

/// Handles `deployment` fires: `depl_ok` and `depl_error` (a recorded
/// business failure) succeed, `depl_down` fails, `depl_slow` outruns the limit.
struct Handler;

#[async_trait]
impl FireHandler for Handler {
    fn kind(&self) -> &'static str {
        "deployment"
    }
    async fn fire(&self, _: &AuthContext, id: &str, _: DateTime<Utc>) -> Result<(), FireError> {
        match id {
            "depl_down" => Err(FireError::Failed("database unavailable".into())),
            "depl_slow" => {
                tokio::time::sleep(RUN_LIMIT + Duration::from_secs(1)).await;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

fn claimed(kind: &str, id: &str, cron: &str, scheduled_at: DateTime<Utc>) -> Claimed {
    let spec = ScheduleSpec {
        cron: cron.into(),
        timezone: "UTC".into(),
        target: FireTarget::new(kind, id),
        tenant: TenantRef { org_id: "o".into(), workspace_id: "w".into(), api_key_id: "k".into() },
    };
    let key = FireKey { workspace_id: "w".into(), target: spec.target.key(), scheduled_at };
    Claimed { key, token: 1, spec: Some(spec) }
}

type Reported = Arc<Mutex<Vec<(Ending, Option<Duration>)>>>;

fn worker(queue: Arc<Queue>) -> (FireWorker, Reported) {
    let registry = Arc::new(FireRegistry::new(queue));
    registry.register(Arc::new(Handler)).unwrap();
    let reported: Reported = Arc::default();
    let sink = reported.clone();
    (FireWorker::new(registry, move |ending, lag| sink.lock().unwrap().push((ending, lag))), reported)
}

/// This hour's occurrence of an hourly schedule: its following one is not due.
fn hourly_now() -> DateTime<Utc> {
    use chrono::DurationRound;
    Utc::now().duration_trunc(chrono::Duration::hours(1)).unwrap()
}

#[tokio::test(start_paused = true)]
async fn each_fire_ends_as_its_run_did() {
    let at = hourly_now();
    let queue = Arc::new(Queue::default());
    let mut unreadable = claimed("deployment", "depl_unreadable", "0 * * * *", at);
    unreadable.spec = None;
    *queue.due.lock().unwrap() = vec![
        claimed("deployment", "depl_ok", "0 * * * *", at),
        claimed("deployment", "depl_error", "0 * * * *", at),
        claimed("deployment", "depl_down", "0 * * * *", at),
        claimed("deployment", "depl_slow", "0 * * * *", at),
        claimed("dream", "drm_nohandler", "0 * * * *", at),
        claimed("deployment", "depl_late", "* * * * *", Utc::now() - chrono::Duration::minutes(5)),
        unreadable,
    ];
    let (worker, reported) = worker(queue.clone());

    assert_eq!(worker.run_due().await.unwrap(), 7);

    let mut finished = queue.finished.lock().unwrap().clone();
    finished.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        finished,
        vec![
            ("deployment/depl_error".into(), Outcome::Ran),
            ("deployment/depl_late".into(), Outcome::Skipped),
            ("deployment/depl_ok".into(), Outcome::Ran),
            ("deployment/depl_unreadable".into(), Outcome::Dropped),
            ("dream/drm_nohandler".into(), Outcome::Dropped),
        ]
    );
    let retried = queue.retried.lock().unwrap().clone();
    let mut targets: Vec<&str> = retried.iter().map(|(t, _)| t.as_str()).collect();
    targets.sort_unstable();
    assert_eq!(targets, ["deployment/depl_down", "deployment/depl_slow"], "an infra failure and a timeout");
    let now = Utc::now();
    assert!(
        retried.iter().all(|(_, again)| *again >= now - chrono::Duration::seconds(1)
            && *again <= now + chrono::Duration::seconds(2)),
        "the first backoff is up to 2 s: {retried:?}"
    );
    let reported = reported.lock().unwrap().clone();
    assert_eq!(reported.iter().filter(|(e, _)| *e == Ending::Retried).count(), 2);
    assert_eq!(reported.iter().filter(|(e, _)| *e == Ending::Done(Outcome::Ran)).count(), 2);
    assert!(
        reported.iter().filter(|(e, _)| *e == Ending::Done(Outcome::Ran)).all(|(_, lag)| lag.is_some()),
        "a run reports its lag"
    );
    assert!(
        reported.iter().filter(|(e, _)| *e == Ending::Done(Outcome::Skipped)).all(|(_, lag)| lag.is_none()),
        "a skipped fire never started"
    );
}

#[tokio::test]
async fn a_worker_whose_lease_was_taken_over_reports_nothing() {
    let queue = Arc::new(Queue { lost: true, ..Queue::default() });
    *queue.due.lock().unwrap() = vec![claimed("deployment", "depl_ok", "0 * * * *", hourly_now())];
    let (worker, reported) = worker(queue.clone());
    worker.run_due().await.unwrap();
    assert_eq!(queue.finished.lock().unwrap().len(), 1, "it tried");
    assert!(reported.lock().unwrap().is_empty());
}

#[test]
fn the_backoff_doubles_from_2_s_to_5_minutes() {
    for (attempt, longest) in [(1, 2_000), (2, 4_000), (3, 8_000), (9, 300_000), (500, 300_000)] {
        let waits: Vec<i64> = (0..200).map(|_| backoff(attempt).num_milliseconds()).collect();
        assert!(waits.iter().all(|w| (0..=longest).contains(w)), "attempt {attempt}: {waits:?}");
        assert!(waits.iter().any(|w| *w > longest / 2), "attempt {attempt}: jittered over its range");
    }
}
