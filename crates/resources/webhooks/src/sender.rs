//! Sending deliveries from their rows (ADR 0001). The change feed's handler
//! only wakes the instances: for each new `webhook_deliveries` row it
//! publishes one message on [`WAKE`] and returns, so a slow endpoint never
//! holds the feed. Every instance runs one claim loop over the table.
//! Delivery is bounded retries with possible duplicate POSTs: a delivery
//! whose outcome is not recorded before its 60 s claim expires is sent again
//! with the same `webhook-id`, unless its claims have lapsed three times.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use shared::BackendError;
use shared::breaker::Breakers;
use shared::net::Guard;
use shared::ports::broadcast::Broadcast;
use shared::ports::changes::{Change, ChangeOp, NativeChangeHandler, Source};
use tokio::sync::watch;

use super::endpoints::Webhooks;
use super::queue::DeliveryQueue;
use super::states::{DisabledReason, EndpointStates};

mod claim;
mod health;
mod send;
#[cfg(test)]
mod tests;

/// The channel a new delivery wakes every instance's claim loop on.
pub const WAKE: &str = "agentd:webhooks";
/// The `user-agent` live's webhooks carry (D22).
const USER_AGENT: &str = "Claude-Platform";

/// The sender's signals; each defaults to nothing.
pub trait SenderMetrics: Send + Sync {
    /// A delivery's outcome was recorded: `delivered`, `retried` or `dropped`.
    fn delivery(&self, _outcome: &'static str) {}
    /// An endpoint's breaker opened on this replica.
    fn breaker_opened(&self) {}
    /// This replica claimed `n` deliveries.
    fn claimed(&self, _n: usize) {}
    /// How long the longest-waiting due delivery has waited, none when nothing is due.
    fn oldest_due(&self, _age: Option<Duration>) {}
    /// The disabled endpoints, by reason.
    fn disabled(&self, _by_reason: &HashMap<DisabledReason, usize>) {}
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoMetrics;

impl SenderMetrics for NoMetrics {}

/// The change feed's handler for `webhook_deliveries`: a new row wakes the senders.
#[derive(Clone)]
pub struct DeliveryWake {
    wakes: Arc<dyn Broadcast>,
}

impl std::fmt::Debug for DeliveryWake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliveryWake").finish_non_exhaustive()
    }
}

impl DeliveryWake {
    pub fn new(wakes: Arc<dyn Broadcast>) -> Self {
        Self { wakes }
    }
}

impl NativeChangeHandler for DeliveryWake {
    fn source(&self) -> Source {
        Source::Table("webhook_deliveries".into())
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        // The claim, the outcome and the sweep's delete are changes too; only an insert wakes.
        let inserted = change.op == ChangeOp::Insert && change.after.as_ref().is_some_and(Value::is_object);
        if !inserted {
            return Ok(());
        }
        // A lost wake costs at most the 1 s poll, so a failed publish is not retried.
        if let Err(e) = self.wakes.publish(WAKE, "").await {
            tracing::warn!(error = %e, "webhooks: a new delivery could not wake the senders; the poll finds it");
        }
        Ok(())
    }
}

/// One instance's sender: the claim loop and the sends it starts.
pub struct WebhookSender {
    claims: claim::ClaimLoop,
}

impl std::fmt::Debug for WebhookSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookSender").finish_non_exhaustive()
    }
}

/// What a sender reads, writes and listens on.
pub struct SenderPorts {
    /// The delivery rows.
    pub queue: Arc<dyn DeliveryQueue>,
    /// The endpoints' shared states.
    pub states: Arc<dyn EndpointStates>,
    /// Where new deliveries wake it.
    pub wakes: Arc<dyn Broadcast>,
    /// What it reports.
    pub metrics: Arc<dyn SenderMetrics>,
}

impl std::fmt::Debug for SenderPorts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SenderPorts").finish_non_exhaustive()
    }
}

impl WebhookSender {
    /// A sender to `webhooks`' endpoints through `guard`, sending live's `user-agent: Claude-Platform`.
    pub fn new(webhooks: Arc<Webhooks>, guard: &Guard, ports: SenderPorts) -> Self {
        let health = Arc::new(health::Health::new(ports.states));
        let send = send::Dispatch {
            guard: guard.with_user_agent(USER_AGENT),
            webhooks,
            queue: ports.queue,
            health,
            metrics: ports.metrics,
            breakers: Breakers::default(),
        };
        Self { claims: claim::ClaimLoop { send: Arc::new(send), wakes: ports.wakes } }
    }

    /// Claim and send until `stop` turns true, then finish the sends in flight.
    pub async fn run(self, stop: watch::Receiver<bool>) {
        self.claims.run(stop).await;
    }
}
