//! scheduler-local — durable-timer backends behind the swappable
//! [`SchedulerPort`] (defined in `shared`).
//!
//! The fire contract is FIXED for every backend (Temporal, EventBridge,
//! Postgres ticker, local):
//! - fire = deliver `(spec, scheduled_at)` to the [`FireRegistry`], which runs
//!   the target's handler under the schedule's tenant,
//! - **attempts = 1, no retry** — a transport failure drops the tick,
//! - **no backfill** — the next occurrence is computed from *now*, missed
//!   ticks during downtime/pause are gone,
//! - exactly-once per `(target, scheduled_at)`.
//!
//! [`LocalScheduler`] is the complete in-process implementation with an
//! injectable clock (`tick(now)`) so firing is deterministic under test.
//! A Temporal backend implements the same port by registering one generic
//! TickWorkflow whose single activity delivers the fire — the port
//! guarantees make it a drop-in.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use shared::ports::fire::FireRegistry;
use shared::ports::{FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort};
use shared::schedule::{CompiledSchedule, CronKind, CronSchedule};

struct Registered {
    spec: ScheduleSpec,
    compiled: CompiledSchedule,
    paused: bool,
    next_fire: Option<DateTime<Utc>>,
    last_fire: Option<DateTime<Utc>>,
}

/// In-process scheduler with a deterministic, injectable clock.
pub struct LocalScheduler {
    fires: Arc<FireRegistry>,
    inner: Mutex<BTreeMap<String, Registered>>,
}

impl std::fmt::Debug for LocalScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalScheduler").finish_non_exhaustive()
    }
}

impl LocalScheduler {
    /// Build over the registry its fires are delivered to.
    pub fn new(fires: Arc<FireRegistry>) -> Self {
        Self { fires, inner: Mutex::new(BTreeMap::new()) }
    }

    /// Advance the clock to `now`, firing everything due. Called by the
    /// background loop in production and directly by tests.
    ///
    /// No-backfill rule: if MULTIPLE occurrences elapsed since the last
    /// tick, only the most recent one fires; earlier misses are dropped.
    pub async fn tick(&self, now: DateTime<Utc>) {
        // Collect due fires under the lock, deliver outside it.
        let mut due: Vec<(ScheduleSpec, DateTime<Utc>)> = Vec::new();
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
                    due.push((reg.spec.clone(), latest));
                    reg.last_fire = Some(latest);
                    // Next occurrence strictly after NOW (no backfill).
                    reg.next_fire = reg.compiled.next_fires(now, 1).first().copied();
                }
            }
        }
        for (spec, scheduled_at) in due {
            // attempts = 1 — errors dropped, matching the contract.
            if let Err(e) = self.fires.fire(&spec, scheduled_at).await {
                tracing::warn!(target = %spec.target.key(), error = %e, "fire dropped (no retry)");
            }
        }
    }

    /// Spawn the background ticking loop (checks every `interval`).
    ///
    /// ⚠️ **Single-instance only.** `LocalScheduler` holds its schedule map
    /// in-process, so running this ticker on more than one agentd replica
    /// makes each replica fire independently. For multi-replica deployments
    /// use the Temporal (or Postgres-claim) `SchedulerPort` backend, whose
    /// timer is a single external source of truth. (The deployment fire path
    /// additionally claims each `(deployment, scheduled_at)` at the store, so
    /// a double-tick can't double-*fire* — but it still wastes work.)
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
            Registered { spec, compiled, paused: false, next_fire: next, last_fire: None },
        );
        Ok(())
    }

    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut g = self.inner.lock();
        let reg = g.get_mut(&target.key()).ok_or_else(|| SchedulerError::Unknown(target.key()))?;
        reg.paused = true;
        Ok(())
    }

    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut g = self.inner.lock();
        let reg = g.get_mut(&target.key()).ok_or_else(|| SchedulerError::Unknown(target.key()))?;
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
        let reg = g.get(&target.key()).ok_or_else(|| SchedulerError::Unknown(target.key()))?;
        Ok(ScheduleState { paused: reg.paused, last_fire: reg.last_fire })
    }
}

#[cfg(test)]
mod tests;
