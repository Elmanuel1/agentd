//! agentd-scheduler — durable-timer backends behind the swappable
//! [`SchedulerPort`] (defined in `agentd-core`).
//!
//! The fire contract is FIXED for every backend (Temporal, EventBridge,
//! Postgres ticker, local):
//! - fire = deliver `(target, scheduled_at)` to the [`FireSink`],
//! - **attempts = 1, no retry** — a transport failure drops the tick,
//! - **no backfill** — the next occurrence is computed from *now*, missed
//!   ticks during downtime/pause are gone,
//! - exactly-once per `(target, scheduled_at)`.
//!
//! [`LocalScheduler`] is the complete in-process implementation with an
//! injectable clock (`tick(now)`) so firing is deterministic under test.
//! A Temporal backend implements the same port by registering one generic
//! TickWorkflow whose single activity POSTs `/internal/fire` — the port
//! guarantees make it a drop-in.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

use std::collections::BTreeMap;
use std::sync::Arc;

use agentd_core::ports::{
    FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort,
};
use agentd_core::schedule::{CompiledSchedule, CronKind, CronSchedule};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;

/// Where fires are delivered. In production this is an HTTP POST to
/// `/internal/fire` (or an in-process call when the scheduler runs inside
/// agentd). Tests use a recording sink.
#[async_trait]
pub trait FireSink: Send + Sync {
    /// Deliver one fire. Errors are DROPPED by the scheduler (no retry).
    async fn fire(&self, target: &FireTarget, scheduled_at: DateTime<Utc>) -> Result<(), FireError>;
}

/// Transport-level fire failure (business failures never reach here — the
/// sink records them and returns Ok, per the contract).
#[derive(Debug, Clone, thiserror::Error)]
#[error("fire failed: {0}")]
pub struct FireError(pub String);

struct Registered {
    spec: ScheduleSpec,
    compiled: CompiledSchedule,
    paused: bool,
    next_fire: Option<DateTime<Utc>>,
    last_fire: Option<DateTime<Utc>>,
}

/// In-process scheduler with a deterministic, injectable clock.
pub struct LocalScheduler {
    sink: Arc<dyn FireSink>,
    inner: Mutex<BTreeMap<String, Registered>>,
}

impl std::fmt::Debug for LocalScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalScheduler").finish_non_exhaustive()
    }
}

impl LocalScheduler {
    /// Build with a fire sink.
    pub fn new(sink: Arc<dyn FireSink>) -> Self {
        Self {
            sink,
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    /// Advance the clock to `now`, firing everything due. Called by the
    /// background loop in production and directly by tests.
    ///
    /// No-backfill rule: if MULTIPLE occurrences elapsed since the last
    /// tick, only the most recent one fires; earlier misses are dropped.
    pub async fn tick(&self, now: DateTime<Utc>) {
        // Collect due fires under the lock, deliver outside it.
        let mut due: Vec<(FireTarget, DateTime<Utc>)> = Vec::new();
        {
            let mut g = self.inner.lock();
            for reg in g.values_mut() {
                if reg.paused {
                    continue;
                }
                let Some(next) = reg.next_fire else { continue };
                if next <= now {
                    // Drop any older missed occurrences: fire only the
                    // LATEST due occurrence (≤ now), then schedule forward.
                    let mut latest = next;
                    loop {
                        match reg.compiled.next_fires(latest, 1).first().copied() {
                            Some(n) if n <= now => latest = n,
                            _ => break,
                        }
                    }
                    due.push((reg.spec.target.clone(), latest));
                    reg.last_fire = Some(latest);
                    // Next occurrence strictly after NOW (no backfill).
                    reg.next_fire = reg.compiled.next_fires(now, 1).first().copied();
                }
            }
        }
        for (target, scheduled_at) in due {
            // attempts = 1 — errors dropped, matching the contract.
            if let Err(e) = self.sink.fire(&target, scheduled_at).await {
                tracing::warn!(target = %target.key(), error = %e, "fire dropped (no retry)");
            }
        }
    }

    /// Spawn the production background loop (checks every `interval`).
    pub fn spawn(self: Arc<Self>, interval: std::time::Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                self.tick(Utc::now()).await;
            }
        })
    }
}

#[async_trait]
impl SchedulerPort for LocalScheduler {
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError> {
        let compiled = CronSchedule {
            kind: CronKind::Cron,
            expression: spec.cron.clone(),
            timezone: spec.timezone.clone(),
        }
        .compile()
        .map_err(|e| SchedulerError::InvalidSpec(e.to_string()))?;
        let next = compiled.next_fires(Utc::now(), 1).first().copied();
        self.inner.lock().insert(
            spec.target.key(),
            Registered {
                spec,
                compiled,
                paused: false,
                next_fire: next,
                last_fire: None,
            },
        );
        Ok(())
    }

    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut g = self.inner.lock();
        let reg = g
            .get_mut(&target.key())
            .ok_or_else(|| SchedulerError::Unknown(target.key()))?;
        reg.paused = true;
        Ok(())
    }

    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut g = self.inner.lock();
        let reg = g
            .get_mut(&target.key())
            .ok_or_else(|| SchedulerError::Unknown(target.key()))?;
        reg.paused = false;
        // Resume from the NEXT occurrence — no backfill of the pause window.
        reg.next_fire = reg.compiled.next_fires(Utc::now(), 1).first().copied();
        Ok(())
    }

    async fn delete(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        self.inner
            .lock()
            .remove(&target.key())
            .map(|_| ())
            .ok_or_else(|| SchedulerError::Unknown(target.key()))
    }

    async fn describe(&self, target: &FireTarget) -> Result<ScheduleState, SchedulerError> {
        let g = self.inner.lock();
        let reg = g
            .get(&target.key())
            .ok_or_else(|| SchedulerError::Unknown(target.key()))?;
        Ok(ScheduleState {
            paused: reg.paused,
            last_fire: reg.last_fire,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PMutex;

    /// Records fires; can be armed to fail (transport error).
    #[derive(Default)]
    struct RecordingSink {
        fires: PMutex<Vec<(String, DateTime<Utc>)>>,
        fail: std::sync::atomic::AtomicBool,
    }

    #[async_trait]
    impl FireSink for RecordingSink {
        async fn fire(
            &self,
            target: &FireTarget,
            scheduled_at: DateTime<Utc>,
        ) -> Result<(), FireError> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(FireError("agentd down".into()));
            }
            self.fires.lock().push((target.key(), scheduled_at));
            Ok(())
        }
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn spec(cron: &str) -> ScheduleSpec {
        ScheduleSpec {
            cron: cron.into(),
            timezone: "UTC".into(),
            target: FireTarget::Deployment {
                id: "depl_01test".into(),
            },
        }
    }

    async fn rig(cron: &str) -> (Arc<LocalScheduler>, Arc<RecordingSink>) {
        let sink = Arc::new(RecordingSink::default());
        let sched = Arc::new(LocalScheduler::new(sink.clone()));
        sched.upsert(spec(cron)).await.unwrap();
        // Deterministic baseline: force next_fire from a known instant.
        {
            let mut g = sched.inner.lock();
            for reg in g.values_mut() {
                reg.next_fire = reg
                    .compiled
                    .next_fires(utc("2026-01-01T00:00:30Z"), 1)
                    .first()
                    .copied();
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
        let fires = sink.fires.lock();
        assert_eq!(fires.len(), 1, "exactly one fire, not five");
        assert_eq!(fires[0].1, utc("2026-01-01T00:25:00Z"), "the LATEST missed one");
        drop(fires);
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
        // means dropped — the schedule already advanced).
        sched.tick(utc("2026-01-01T00:05:30Z")).await;
        assert!(sink.fires.lock().is_empty());
        // The NEXT occurrence fires normally.
        sched.tick(utc("2026-01-01T00:10:00Z")).await;
        assert_eq!(sink.fires.lock().len(), 1);
    }

    #[tokio::test]
    async fn pause_suppresses_unpause_resumes_without_backfill() {
        let (sched, sink) = rig("*/5 * * * *").await;
        let target = FireTarget::Deployment {
            id: "depl_01test".into(),
        };
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
        let target = FireTarget::Deployment {
            id: "depl_01test".into(),
        };
        let st = sched.describe(&target).await.unwrap();
        assert!(!st.paused);
        assert!(st.last_fire.is_none());
        sched.delete(&target).await.unwrap();
        assert!(matches!(
            sched.describe(&target).await,
            Err(SchedulerError::Unknown(_))
        ));
    }

    #[tokio::test]
    async fn upsert_rejects_bad_cron_via_shared_engine() {
        let sink = Arc::new(RecordingSink::default());
        let sched = LocalScheduler::new(sink);
        let mut s = spec("99 * * * *");
        assert!(matches!(
            sched.upsert(s.clone()).await,
            Err(SchedulerError::InvalidSpec(_))
        ));
        s.cron = "0 0 * * *".into();
        s.timezone = "Mars/Olympus".into();
        assert!(matches!(
            sched.upsert(s).await,
            Err(SchedulerError::InvalidSpec(_))
        ));
    }

    #[tokio::test]
    async fn last_fire_is_recorded() {
        let (sched, _sink) = rig("*/5 * * * *").await;
        sched.tick(utc("2026-01-01T00:05:00Z")).await;
        let target = FireTarget::Deployment {
            id: "depl_01test".into(),
        };
        assert_eq!(
            sched.describe(&target).await.unwrap().last_fire,
            Some(utc("2026-01-01T00:05:00Z"))
        );
    }
}
