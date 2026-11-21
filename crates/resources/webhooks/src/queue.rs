//! The delivery queue: `webhook_deliveries` rows claimed by one replica at a
//! time for 60 s. A claim's token is the row's `claim_seq`, which every claim
//! raises and nothing lowers, so a write from a claim a newer one replaced
//! changes nothing (D26).

use std::future::Future;
use std::time::Duration;

use shared::ports::BoxFuture;
use shared::{BackendError, Timestamp};

/// The claim a write is fenced on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRef {
    /// The event's id, the `webhook-id` header.
    pub webhook_id: String,
    /// The endpoint it goes to.
    pub endpoint_key: String,
    /// The row's `claim_seq` this claim set.
    pub claim_seq: i64,
}

/// A delivery this replica holds the claim of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The claim.
    pub claim: ClaimRef,
    /// POSTs counted so far, a lapsed claim's among them; at 3 it is dropped unsent.
    pub attempt: i64,
    /// Where it is posted.
    pub endpoint_url: String,
    /// The exact bytes signed and sent.
    pub body: String,
}

/// What an attempt's answer makes of a delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Answered 2xx.
    Delivered,
    /// Sent again after the wait.
    Retry(Duration),
    /// Its last attempt failed.
    Dropped,
}

/// One POST's answer, recorded on the row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempted {
    /// What becomes of the delivery.
    pub next: Next,
    /// The status answered, when one was.
    pub status: Option<u16>,
    /// Why it failed, when it did.
    pub error: Option<String>,
}

/// The queue, in durable storage. Each write returns `false` when a newer claim holds the row.
pub trait DeliveryQueue: Send + Sync {
    /// Claim up to `limit` due deliveries for 60 s, none to the endpoints in `skip`.
    fn claim<'a>(
        &'a self,
        limit: usize,
        skip: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Claimed>, BackendError>>;
    /// Give the delivery back unsent, spending no attempt.
    fn release<'a>(&'a self, claim: &'a ClaimRef) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// Drop the delivery without sending it.
    fn drop_unsent<'a>(
        &'a self,
        claim: &'a ClaimRef,
        why: &'a str,
    ) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// Record one POST's answer, counting the attempt.
    fn record<'a>(
        &'a self,
        claim: &'a ClaimRef,
        attempted: &'a Attempted,
    ) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// When the longest-waiting delivery not delivered or dropped fell due.
    fn oldest_due(&self) -> BoxFuture<'_, Result<Option<Timestamp>, BackendError>>;
}

/// What an adapter implements to provide [`DeliveryQueue`]; the methods are documented there.
pub trait NativeDeliveryQueue: Send + Sync {
    fn claim(
        &self,
        limit: usize,
        skip: &[String],
    ) -> impl Future<Output = Result<Vec<Claimed>, BackendError>> + Send;
    fn release(&self, claim: &ClaimRef) -> impl Future<Output = Result<bool, BackendError>> + Send;
    fn drop_unsent(
        &self,
        claim: &ClaimRef,
        why: &str,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send;
    fn record(
        &self,
        claim: &ClaimRef,
        attempted: &Attempted,
    ) -> impl Future<Output = Result<bool, BackendError>> + Send;
    fn oldest_due(&self) -> impl Future<Output = Result<Option<Timestamp>, BackendError>> + Send;
}

impl<T: NativeDeliveryQueue> DeliveryQueue for T {
    fn claim<'a>(
        &'a self,
        limit: usize,
        skip: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Claimed>, BackendError>> {
        Box::pin(NativeDeliveryQueue::claim(self, limit, skip))
    }
    fn release<'a>(&'a self, claim: &'a ClaimRef) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(NativeDeliveryQueue::release(self, claim))
    }
    fn drop_unsent<'a>(
        &'a self,
        claim: &'a ClaimRef,
        why: &'a str,
    ) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(NativeDeliveryQueue::drop_unsent(self, claim, why))
    }
    fn record<'a>(
        &'a self,
        claim: &'a ClaimRef,
        attempted: &'a Attempted,
    ) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(NativeDeliveryQueue::record(self, claim, attempted))
    }
    fn oldest_due(&self) -> BoxFuture<'_, Result<Option<Timestamp>, BackendError>> {
        Box::pin(NativeDeliveryQueue::oldest_due(self))
    }
}
