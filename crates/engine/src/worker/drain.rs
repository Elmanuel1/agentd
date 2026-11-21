//! Claiming until the stop signal, then draining: at SIGTERM the worker
//! claims nothing more and waits for its running steps until the grace ends;
//! the steps still running then are stopped and released in one statement,
//! due at once, so another replica takes them instead of waiting out their
//! leases (SH-7, SH-8).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sessions::steps::Claim;
use tokio::sync::{Semaphore, watch};
use tokio::task::{Id, JoinSet};
use tokio::time::Instant;

use super::Loop;

/// How long an idle worker waits for a signal before it looks again.
const IDLE: Duration = Duration::from_secs(1);
/// The part of the grace kept for the release statement.
const RELEASE: Duration = Duration::from_secs(1);

impl Loop {
    /// Claim and run steps until `stop` turns true, then drain inside the grace.
    pub fn spawn(self, stop: watch::Receiver<bool>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(Arc::new(self).serve(stop))
    }

    async fn serve(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        let permits = Arc::new(Semaphore::new(self.settings.steps));
        let mut woken = self.signals.subscribe();
        let mut running = JoinSet::new();
        let mut held: HashMap<Id, Claim> = HashMap::new();
        let mut unstarted = None;
        while !*stop.borrow() {
            while let Some(done) = running.try_join_next_with_id() {
                held.remove(&done.map_or_else(|e| e.id(), |(id, ())| id));
            }
            let permit = tokio::select! {
                _ = stop.changed() => break,
                permit = permits.clone().acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => break,
                },
            };
            let s = &self.settings;
            let claim = match self.stores.steps.claim(&s.owner, s.lease_seconds(), s.per_workspace).await {
                // A claim that returns after the stop is released with the running steps, not started.
                Ok(Some(claim)) if *stop.borrow() => {
                    unstarted = Some(claim);
                    break;
                }
                Ok(Some(claim)) => claim,
                Ok(None) => {
                    drop(permit);
                    tokio::select! {
                        _ = stop.changed() => break,
                        _ = tokio::time::timeout(IDLE, woken.recv()) => {}
                    }
                    continue;
                }
                Err(e) => {
                    drop(permit);
                    tracing::warn!(error = %e, "loop: could not claim a step");
                    tokio::time::sleep(IDLE).await;
                    continue;
                }
            };
            let worker = self.clone();
            let step = claim.clone();
            let id = running.spawn(async move {
                worker.step(step).await;
                drop(permit);
            });
            held.insert(id.id(), claim);
        }
        self.drain(running, held, unstarted).await;
    }

    async fn drain(&self, mut running: JoinSet<()>, mut held: HashMap<Id, Claim>, unstarted: Option<Claim>) {
        let deadline = Instant::now() + self.settings.grace.saturating_sub(RELEASE);
        loop {
            match tokio::time::timeout_at(deadline, running.join_next_with_id()).await {
                Ok(None) => break,
                Ok(Some(done)) => {
                    held.remove(&done.map_or_else(|e| e.id(), |(id, ())| id));
                }
                Err(_) => break,
            }
        }
        running.abort_all();
        // Each aborted step stops at its next await; none may still be writing once another replica can claim it.
        while running.join_next().await.is_some() {}
        let left: Vec<Claim> = held.into_values().chain(unstarted).collect();
        if left.is_empty() {
            return;
        }
        match self.stores.steps.release_all(&self.settings.owner, &left).await {
            Ok(n) => {
                crate::metrics::released(n);
                tracing::info!(
                    steps = n,
                    "loop: the grace ended; the running steps were released, due at once"
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "loop: the running steps could not be released; they wait out their leases")
            }
        }
    }
}
