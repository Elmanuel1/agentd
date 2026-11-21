//! The claim loop every instance runs: on a wake, and every second without
//! one, claim up to its free senders (at most 100) of due deliveries, at most
//! once every 50 ms, and send each in the background. Wakes that arrive in
//! between fold into the next claim. Postgres gives each row to one instance
//! (`FOR NO KEY UPDATE SKIP LOCKED`), so each instance added adds senders.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use shared::Timestamp;
use tokio::sync::{Notify, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::WAKE;
use super::health::Health;
use super::send::Dispatch;
use crate::endpoints::Webhooks;
use crate::states::Configured;

/// Sends in flight per instance: a bound on one process, not a fairness limit.
pub(crate) const SENDERS: usize = 256;
/// Deliveries one claim takes, at most.
const BATCH: usize = 100;
/// The poll behind the wake (D18).
const POLL: Duration = Duration::from_secs(1);
/// The least time between two claims.
const FOLD: Duration = Duration::from_millis(50);
/// A claim's length, `claimed_until` minus its start.
const CLAIM: Duration = Duration::from_secs(60);
/// How often the endpoint states and the oldest due delivery are read.
const READ_EVERY: Duration = Duration::from_secs(10);

/// One instance's claim loop.
pub(crate) struct ClaimLoop {
    pub(crate) send: Arc<Dispatch>,
    pub(crate) wakes: Arc<dyn shared::ports::broadcast::Broadcast>,
}

impl ClaimLoop {
    /// Claim and send until `stop` turns true, then wait for the sends in flight.
    pub(crate) async fn run(self, mut stop: watch::Receiver<bool>) {
        let woken = Arc::new(Notify::new());
        let listening = tokio::spawn(listen(self.wakes.subscribe(WAKE), woken.clone()));
        let reading = tokio::spawn(read_states(self.send.clone()));
        let mut poll = tokio::time::interval(POLL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut sending = JoinSet::new();
        while !*stop.borrow() {
            tokio::select! {
                _ = stop.changed() => break,
                () = woken.notified() => {}
                _ = poll.tick() => {}
                Some(_) = sending.join_next(), if sending.len() >= SENDERS => {}
            }
            while sending.try_join_next().is_some() {}
            if self.claim(&mut sending).await {
                woken.notify_one();
            }
            tokio::select! {
                _ = stop.changed() => break,
                () = tokio::time::sleep(FOLD) => {}
            }
        }
        listening.abort();
        reading.abort();
        while sending.join_next().await.is_some() {}
    }

    /// One claim into the free senders; `true` when it filled them all, so more may be due.
    async fn claim(&self, sending: &mut JoinSet<()>) -> bool {
        let free = SENDERS.saturating_sub(sending.len()).min(BATCH);
        if free == 0 || !self.send.health.fresh() {
            return false;
        }
        let health = &self.send.health;
        let skip: Vec<String> =
            self.send.breakers.held().into_iter().filter(|k| !health.disabled(k)).collect();
        let deadline = Instant::now() + CLAIM;
        let claimed = match self.send.queue.claim(free, &skip).await {
            Ok(claimed) => claimed,
            Err(e) => {
                tracing::warn!(error = %e, "webhooks: the delivery queue could not be claimed from");
                return false;
            }
        };
        self.send.metrics.claimed(claimed.len());
        let full = claimed.len() == free;
        for c in claimed {
            let send = self.send.clone();
            sending.spawn(async move { send.deliver(c, deadline).await });
        }
        full
    }
}

/// Wake the loop on each message on [`WAKE`]; many fold into one.
async fn listen(mut wakes: futures::stream::BoxStream<'static, String>, woken: Arc<Notify>) {
    while wakes.next().await.is_some() {
        woken.notify_one();
    }
}

/// Every 10 s: store the configured endpoints until that succeeds once,
/// read the endpoint states, and report the oldest due delivery and the
/// disabled endpoints.
async fn read_states(send: Arc<Dispatch>) {
    let configured: Vec<Configured> = send.webhooks.all().map(crate::Endpoint::configured).collect();
    let mut stored = false;
    let mut every = tokio::time::interval(READ_EVERY);
    loop {
        every.tick().await;
        if !stored {
            stored = configure(&send.webhooks, &send.health, &configured).await;
        }
        if let Err(e) = send.health.refresh().await {
            tracing::warn!(error = %e, "webhooks: the endpoint states could not be read; no POST starts until they are");
        }
        send.metrics.disabled(&send.health.disabled_by_reason());
        match send.queue.oldest_due().await {
            Ok(due) => send.metrics.oldest_due(due.map(|due| {
                (Timestamp::now().as_datetime() - due.as_datetime()).to_std().unwrap_or_default()
            })),
            Err(e) => tracing::warn!(error = %e, "webhooks: the oldest due delivery could not be read"),
        }
    }
}

async fn configure(webhooks: &Webhooks, health: &Health, configured: &[Configured]) -> bool {
    if webhooks.is_empty() {
        return true;
    }
    match health.states().configure(configured).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(error = %e, "webhooks: the configured endpoints could not be stored; trying again");
            false
        }
    }
}
