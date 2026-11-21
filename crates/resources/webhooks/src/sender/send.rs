//! One claimed delivery, start to end: dropped unsent when its endpoint is
//! gone or disabled or its claims lapsed three times; released unsent when
//! this replica's endpoint states are stale, less than 15 s of its claim
//! remain or its endpoint's breaker holds it; otherwise signed and posted
//! through the egress guard, and its answer recorded on the row and on the
//! endpoint's health.

use std::sync::Arc;
use std::time::Duration;

use rand::RngExt;
use shared::Timestamp;
use shared::breaker::{Breakers, Pass};
use shared::net::{Guard, NetError};
use tokio::time::Instant;

use super::SenderMetrics;
use super::health::Health;
use crate::endpoints::{Endpoint, Webhooks};
use crate::queue::{Attempted, ClaimRef, Claimed, DeliveryQueue, Next};
use crate::sign::sign;
use crate::states::DisabledReason;

/// The attempts a delivery gets (L1).
pub(crate) const ATTEMPTS: i64 = 3;
/// How long one POST may take.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The claim a POST must still have when it starts: the timeout and 5 s spare.
const MARGIN: Duration = Duration::from_secs(15);
/// The first retry's base wait; it doubles per attempt, half fixed and half random.
const BACKOFF: Duration = Duration::from_secs(30);

/// How an attempt ended, before it is recorded.
enum Answer {
    Ok(u16),
    Redirect(u16),
    Failed(Option<u16>, String),
}

/// Sends claimed deliveries.
pub(crate) struct Dispatch {
    pub(crate) guard: Guard,
    pub(crate) webhooks: Arc<Webhooks>,
    pub(crate) queue: Arc<dyn DeliveryQueue>,
    pub(crate) health: Arc<Health>,
    pub(crate) metrics: Arc<dyn SenderMetrics>,
    pub(crate) breakers: Breakers<String>,
}

impl Dispatch {
    /// Deliver `c`, whose claim this replica holds until `deadline`.
    pub(crate) async fn deliver(&self, c: Claimed, deadline: Instant) {
        let key = c.claim.endpoint_key.as_str();
        if c.attempt >= ATTEMPTS {
            return self.drop_unsent(&c.claim, "its claims lapsed three times").await;
        }
        let Some(endpoint) = self.webhooks.endpoint(key) else {
            return self.drop_unsent(&c.claim, "the endpoint is not configured").await;
        };
        if self.health.disabled(key) {
            return self.drop_unsent(&c.claim, "the endpoint is disabled").await;
        }
        let url = reqwest::Url::parse(&c.endpoint_url);
        let client = match url.as_ref().map_err(|_| NetError::Url) {
            Ok(url) => self.guard.client(url, TIMEOUT).await,
            Err(e) => Err(e),
        };
        if let Err(NetError::Internal(addr)) = &client {
            self.disable(endpoint, DisabledReason::PrivateAddress, addr).await;
            return self.drop_unsent(&c.claim, "the endpoint resolves to a non-public address").await;
        }
        let ready = self.health.fresh() && deadline.saturating_duration_since(Instant::now()) >= MARGIN;
        if !ready || self.breakers.pass(&key.to_owned()) != Pass::Go {
            return self.release(&c.claim).await;
        }
        let answer = match client {
            Ok(client) => post(&client, &c, endpoint).await,
            Err(e) => Answer::Failed(None, e.to_string()),
        };
        self.answered(&c, endpoint, answer).await;
    }

    async fn answered(&self, c: &Claimed, endpoint: &Endpoint, answer: Answer) {
        let key = &c.claim.endpoint_key;
        let attempt = c.attempt + 1;
        let failed = |status, error| Attempted { next: retry_or_drop(attempt), status, error: Some(error) };
        let attempted = match answer {
            Answer::Ok(status) => {
                self.breakers.succeeded(key);
                Attempted { next: Next::Delivered, status: Some(status), error: None }
            }
            Answer::Redirect(status) => {
                self.disable(endpoint, DisabledReason::Redirect, &status.to_string()).await;
                failed(Some(status), format!("answered {status}"))
            }
            Answer::Failed(status, error) => {
                self.failed(endpoint).await;
                failed(status, error)
            }
        };
        match self.queue.record(&c.claim, &attempted).await {
            Ok(true) => self.metrics.delivery(attempted.next.outcome()),
            Ok(false) => {
                tracing::info!(webhook_id = %c.claim.webhook_id, "webhooks: a newer claim holds the delivery; its outcome is not recorded")
            }
            Err(e) => {
                tracing::warn!(webhook_id = %c.claim.webhook_id, error = %e, "webhooks: a delivery's outcome could not be recorded; it is sent again after its claim")
            }
        }
        if attempted.next == Next::Dropped {
            tracing::info!(webhook_id = %c.claim.webhook_id, endpoint = %key, "webhooks: a delivery was dropped after its attempts");
        }
    }

    async fn failed(&self, endpoint: &Endpoint) {
        if self.breakers.failed(&endpoint.key, None) {
            self.metrics.breaker_opened();
        }
        match self.health.failed(&endpoint.configured()).await {
            Ok(true) => disabled_log(endpoint, DisabledReason::Failing, "failing for 120 h"),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(endpoint = %endpoint.key, error = %e, "webhooks: a failed attempt could not be counted on its endpoint")
            }
        }
    }

    async fn disable(&self, endpoint: &Endpoint, why: DisabledReason, detail: &str) {
        match self.health.disable(&endpoint.configured(), why).await {
            Ok(true) => disabled_log(endpoint, why, detail),
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(endpoint = %endpoint.key, error = %e, "webhooks: an endpoint could not be disabled")
            }
        }
    }

    async fn drop_unsent(&self, claim: &ClaimRef, why: &str) {
        match self.queue.drop_unsent(claim, why).await {
            Ok(true) => {
                self.metrics.delivery("dropped");
                tracing::info!(webhook_id = %claim.webhook_id, endpoint = %claim.endpoint_key, why, "webhooks: a delivery was dropped unsent");
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(webhook_id = %claim.webhook_id, error = %e, "webhooks: a delivery could not be dropped; its claim expires")
            }
        }
    }

    async fn release(&self, claim: &ClaimRef) {
        if let Err(e) = self.queue.release(claim).await {
            tracing::warn!(webhook_id = %claim.webhook_id, error = %e, "webhooks: a delivery could not be released; its claim expires");
        }
    }
}

/// A person must act: the endpoint gets nothing until it is enabled again (SH-43).
fn disabled_log(endpoint: &Endpoint, why: DisabledReason, detail: &str) {
    tracing::error!(org = %endpoint.organization_id, endpoint = %endpoint.key, reason = why.as_str(), detail, "webhooks: an endpoint was disabled; `agentd webhooks enable` sends to it again");
}

async fn post(client: &reqwest::Client, c: &Claimed, endpoint: &Endpoint) -> Answer {
    let id = c.claim.webhook_id.as_str();
    let ts = Timestamp::now().as_datetime().timestamp();
    let sent = client
        .post(&c.endpoint_url)
        .header("content-type", "application/json")
        .header("webhook-id", id)
        .header("webhook-timestamp", ts.to_string())
        .header("webhook-signature", sign(&endpoint.secret, id, ts, &c.body))
        .body(c.body.clone())
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => Answer::Ok(r.status().as_u16()),
        Ok(r) if r.status().is_redirection() => Answer::Redirect(r.status().as_u16()),
        Ok(r) => Answer::Failed(Some(r.status().as_u16()), format!("answered {}", r.status().as_u16())),
        Err(e) => Answer::Failed(None, e.to_string()),
    }
}

/// After failed attempt `attempt`: dropped at the third, else a wait of
/// 15 to 30 s after the first and 30 to 60 s after the second (SH-3).
pub(crate) fn retry_or_drop(attempt: i64) -> Next {
    if attempt >= ATTEMPTS {
        return Next::Dropped;
    }
    let doublings = u32::try_from(attempt.saturating_sub(1)).unwrap_or(0);
    let base = BACKOFF.saturating_mul(2u32.saturating_pow(doublings));
    let half = base / 2;
    Next::Retry(half + half.mul_f64(rand::rng().random_range(0.0..=1.0)))
}
