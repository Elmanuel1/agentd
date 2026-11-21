//! The step queue: one row per session that has work for the loop. A loop
//! worker claims a due row under a lease, runs one model call from the log
//! and commits its events, so a replica holds nothing between steps and a
//! step cut off by a crash runs again from the log after its lease.

use shared::WorkspaceId;
use shared::ports::BoxFuture;
use std::future::Future;

use crate::events::{Event, EventId};
use crate::store::StoreError;
use crate::types::{Session, SessionId};

/// A step a worker holds. A write that names it lands only while the worker
/// still holds the step with these counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The session's workspace.
    pub workspace: WorkspaceId,
    /// The session.
    pub session: SessionId,
    /// The worker that holds it.
    pub owner: String,
    /// The claims the step has had, this one included.
    pub attempts: i64,
    /// The wakes the step had when this claim took it.
    pub wakes: i64,
    /// How long the step waited past its due time before this claim.
    pub waited_ms: i64,
}

/// Changes a step makes to its session's row; `false` when nothing changed.
pub type SessionChange<'a> = dyn Fn(&mut Session) -> bool + Send + Sync + 'a;

/// The webhook deliveries of the session's status becoming the given one,
/// for the given organization.
pub type StatusDeliveries<'a> =
    dyn Fn(&str, crate::types::SessionStatus) -> Vec<webhooks::Delivery> + Send + Sync + 'a;

/// What one write of a held step lands, in one transaction.
pub struct StepWrite<'a> {
    /// The client events the step read, stamped processed at `at`.
    pub read: &'a [EventId],
    /// When they were processed, ordered among the events' own times.
    pub at: shared::Timestamp,
    /// The events it appends, in order.
    pub events: &'a [Event],
    /// What the loop keeps beside some of those events, by event id, which
    /// no API read serves.
    pub engine: &'a [(EventId, serde_json::Value)],
    /// What it changes on the session's row.
    pub session: &'a SessionChange<'a>,
    /// The webhook deliveries of a status change, when any endpoint is configured.
    pub deliveries: Option<&'a StatusDeliveries<'a>>,
}

impl std::fmt::Debug for StepWrite<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepWrite")
            .field("read", &self.read)
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

/// How a step ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEnd {
    /// Nothing is left to run until the session is woken again.
    Done,
    /// Another step runs at once (the turn goes on).
    Again,
    /// Done, and the session's stopped work item is queued again, for a
    /// worker to run the built-in calls the step parked.
    Requeue,
}

/// The rows the queue holds, as the gauges count them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepCounts {
    /// Due and held by no live lease.
    pub due: i64,
    /// Held under a lease that has not run out.
    pub leased: i64,
}

/// The step queue.
pub trait Steps: Send + Sync {
    /// The oldest due step, held by `owner` for `lease` seconds; `None` when
    /// none is due, or its workspace is at `per_workspace` held steps, which
    /// defers the workspace's waiting steps by a second.
    fn claim<'a>(
        &'a self,
        owner: &'a str,
        lease: i64,
        per_workspace: i64,
    ) -> BoxFuture<'a, Result<Option<Claim>, StoreError>>;
    /// Under the session's append lock, write `write` while `claim` still
    /// holds the step, and keep its lease for `lease` more seconds; `false`
    /// when it no longer does, with nothing written.
    fn write<'a>(
        &'a self,
        claim: &'a Claim,
        lease: i64,
        write: StepWrite<'a>,
    ) -> BoxFuture<'a, Result<bool, StoreError>>;
    /// Write the step's last events and release it as `end` says; `false`
    /// when another worker took it over, with nothing written.
    fn commit<'a>(
        &'a self,
        claim: &'a Claim,
        end: StepEnd,
        write: StepWrite<'a>,
    ) -> BoxFuture<'a, Result<bool, StoreError>>;
    /// Release the step unrun, due again in `seconds`.
    fn defer<'a>(&'a self, claim: &'a Claim, seconds: i64) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Release the failed step unrun, after a random wait that doubles with its failures in a row.
    fn defer_failed<'a>(&'a self, claim: &'a Claim) -> BoxFuture<'a, Result<(), StoreError>>;
    /// Release every one of `claims` `owner` still holds, due at once; how many.
    fn release_all<'a>(
        &'a self,
        owner: &'a str,
        claims: &'a [Claim],
    ) -> BoxFuture<'a, Result<u64, StoreError>>;
    /// The queue's rows, counted.
    fn counts(&self) -> BoxFuture<'_, Result<StepCounts, StoreError>>;
}

/// What an adapter implements to provide [`Steps`]; the methods are documented there.
pub trait NativeSteps: Send + Sync {
    fn claim(
        &self,
        owner: &str,
        lease: i64,
        per_workspace: i64,
    ) -> impl Future<Output = Result<Option<Claim>, StoreError>> + Send;
    fn write(
        &self,
        claim: &Claim,
        lease: i64,
        write: StepWrite<'_>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
    fn commit(
        &self,
        claim: &Claim,
        end: StepEnd,
        write: StepWrite<'_>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
    fn defer(&self, claim: &Claim, seconds: i64) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn defer_failed(&self, claim: &Claim) -> impl Future<Output = Result<(), StoreError>> + Send;
    fn release_all(
        &self,
        owner: &str,
        claims: &[Claim],
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
    fn counts(&self) -> impl Future<Output = Result<StepCounts, StoreError>> + Send;
}

impl<T: NativeSteps> Steps for T {
    fn claim<'a>(
        &'a self,
        owner: &'a str,
        lease: i64,
        per_workspace: i64,
    ) -> BoxFuture<'a, Result<Option<Claim>, StoreError>> {
        Box::pin(NativeSteps::claim(self, owner, lease, per_workspace))
    }
    fn write<'a>(
        &'a self,
        claim: &'a Claim,
        lease: i64,
        write: StepWrite<'a>,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(NativeSteps::write(self, claim, lease, write))
    }
    fn commit<'a>(
        &'a self,
        claim: &'a Claim,
        end: StepEnd,
        write: StepWrite<'a>,
    ) -> BoxFuture<'a, Result<bool, StoreError>> {
        Box::pin(NativeSteps::commit(self, claim, end, write))
    }
    fn defer<'a>(&'a self, claim: &'a Claim, seconds: i64) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeSteps::defer(self, claim, seconds))
    }
    fn defer_failed<'a>(&'a self, claim: &'a Claim) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(NativeSteps::defer_failed(self, claim))
    }
    fn release_all<'a>(
        &'a self,
        owner: &'a str,
        claims: &'a [Claim],
    ) -> BoxFuture<'a, Result<u64, StoreError>> {
        Box::pin(NativeSteps::release_all(self, owner, claims))
    }
    fn counts(&self) -> BoxFuture<'_, Result<StepCounts, StoreError>> {
        Box::pin(NativeSteps::counts(self))
    }
}
