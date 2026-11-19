//! The fire workers: every replica claims due fires from the queue, runs each
//! through the [`FireRegistry`] within a time limit, and marks it done or
//! frees it to run again after a backoff, until its following occurrence is
//! due.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use rand::RngExt;
use tokio::task::{JoinHandle, JoinSet};

use super::{Claimed, FireError, FireRegistry, Outcome, jitter};

/// Fires one replica runs at once.
const CONCURRENT: usize = 16;
/// A fire still running after this has failed.
const RUN_LIMIT: Duration = Duration::from_secs(60);
/// A settlement write still waiting after this is left for the lease to expire.
const SETTLE_LIMIT: Duration = Duration::from_secs(10);
/// The first retry's longest backoff, in milliseconds; it doubles per attempt.
const BACKOFF_FIRST_MS: u64 = 2_000;
/// The longest backoff, in milliseconds.
const BACKOFF_LAST_MS: u64 = 300_000;

/// What became of one claimed fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It is done, for `Outcome`.
    Done(Outcome),
    /// It failed and runs again after its backoff.
    Retried,
}

type Report = Arc<dyn Fn(Ending, Option<Duration>) + Send + Sync>;

/// Runs the due fires of the queue.
#[derive(Clone)]
pub struct FireWorker {
    registry: Arc<FireRegistry>,
    report: Report,
}

impl std::fmt::Debug for FireWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FireWorker").finish_non_exhaustive()
    }
}

impl FireWorker {
    /// Running fires through `registry`; `report` hears each fire's ending,
    /// with its lag (its start minus its due time) when it ran.
    pub fn new(
        registry: Arc<FireRegistry>,
        report: impl Fn(Ending, Option<Duration>) + Send + Sync + 'static,
    ) -> Self {
        Self { registry, report: Arc::new(report) }
    }

    /// Claim up to 16 due fires and run them at once; how many it claimed.
    pub async fn run_due(&self) -> Result<usize, FireError> {
        let claimed = self.registry.claims.claim_due(CONCURRENT).await?;
        futures::future::join_all(claimed.iter().map(|c| self.settle(c))).await;
        Ok(claimed.len())
    }

    /// Run due fires, at most 16 at once: claim as many as there are free
    /// slots every `interval` and whenever a fire finishes, and again at once
    /// while a claim fills every free slot.
    pub fn spawn(self, interval: Duration) -> JoinHandle<()> {
        tokio::spawn(async move { self.serve(interval).await })
    }

    async fn serve(&self, interval: Duration) {
        let mut ticker = tokio::time::interval(interval);
        let mut running = JoinSet::new();
        loop {
            while running.try_join_next().is_some() {}
            let free = CONCURRENT.saturating_sub(running.len());
            let claimed = match self.registry.claims.claim_due(free).await {
                Ok(claimed) => claimed,
                Err(e) => {
                    tracing::warn!(error = %e, "fires: the fire queue cannot be read");
                    Vec::new()
                }
            };
            let full = free > 0 && claimed.len() == free;
            for c in claimed {
                let worker = self.clone();
                running.spawn(async move { worker.settle(&c).await });
            }
            if full {
                continue;
            }
            tokio::select! {
                Some(_) = running.join_next(), if !running.is_empty() => {}
                _ = ticker.tick() => {}
            }
        }
    }

    /// Run `c` once, unless it can no longer run, and record its ending.
    async fn settle(&self, c: &Claimed) {
        let (target, at) = (&c.key.target, c.key.scheduled_at);
        let Some(spec) = c.spec.as_ref() else {
            tracing::error!(%target, %at, "fires: a queued fire's spec cannot be read; dropped");
            return self.end(c, Ending::Done(Outcome::Dropped), None).await;
        };
        let now = Utc::now();
        if jitter::following(spec, at).is_some_and(|next| next <= now) {
            tracing::warn!(%target, %at, "fires: a fire did not run before its following occurrence; skipped");
            return self.end(c, Ending::Done(Outcome::Skipped), None).await;
        }
        let lag = (now - jitter::due_at(spec, at)).to_std().unwrap_or_default();
        let ending = match tokio::time::timeout(RUN_LIMIT, self.registry.run(spec, at)).await {
            Ok(Ok(())) => Ending::Done(Outcome::Ran),
            Ok(Err(FireError::NoHandler(kind))) => {
                tracing::error!(%target, %at, %kind, "fires: no handler for the fire's kind; dropped");
                Ending::Done(Outcome::Dropped)
            }
            Ok(Err(e)) => {
                tracing::warn!(%target, %at, attempt = c.token, error = %e, "fires: a fire failed; it runs again after its backoff");
                Ending::Retried
            }
            Err(_) => {
                tracing::warn!(%target, %at, attempt = c.token, "fires: a fire ran past its limit; it runs again after its backoff");
                Ending::Retried
            }
        };
        let started = (ending != Ending::Done(Outcome::Dropped)).then_some(lag);
        self.end(c, ending, started).await
    }

    async fn end(&self, c: &Claimed, ending: Ending, lag: Option<Duration>) {
        let write = match ending {
            Ending::Done(outcome) => self.registry.claims.finish(c, outcome),
            Ending::Retried => self.registry.claims.retry(c, Utc::now() + backoff(c.token)),
        };
        let written = tokio::time::timeout(SETTLE_LIMIT, write)
            .await
            .unwrap_or_else(|_| Err(FireError::Unavailable("the write ran past its limit".into())));
        let (target, at) = (&c.key.target, c.key.scheduled_at);
        match written {
            Ok(true) => (self.report)(ending, lag),
            Ok(false) => {
                tracing::warn!(%target, %at, token = c.token, "fires: another worker took the fire's lease over; left as it is");
            }
            Err(e) => {
                tracing::warn!(%target, %at, error = %e, "fires: a fire's claim cannot be written; it runs again after its lease");
            }
        }
    }
}

/// A random wait up to 2 s doubled per earlier attempt, at most 5 minutes.
fn backoff(attempt: i32) -> chrono::Duration {
    let doublings = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0).min(20);
    let longest = BACKOFF_FIRST_MS.saturating_mul(1 << doublings).min(BACKOFF_LAST_MS);
    let ms = rand::rng().random_range(0..=longest);
    chrono::Duration::milliseconds(i64::try_from(ms).unwrap_or(0))
}

#[cfg(test)]
mod tests;
