//! The fire queue: every occurrence a scheduler delivers (a due schedule, a
//! redelivered stream entry) is one row, keyed by its workspace, target and
//! time, written once however often it arrives. Workers claim the due rows
//! under a lease; a claim's token is the row's attempt count, so a worker
//! whose lease another took over writes nothing.

use std::future::Future;

use chrono::{DateTime, Utc};

use super::FireError;
use crate::ports::{BoxFuture, ScheduleSpec};

/// One occurrence of a schedule, due to run at or after `due_at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fire {
    /// The schedule.
    pub spec: ScheduleSpec,
    /// The occurrence's listed time.
    pub scheduled_at: DateTime<Utc>,
    /// The earliest moment it may run.
    pub due_at: DateTime<Utc>,
}

/// The row of one occurrence in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireKey {
    /// The schedule's workspace.
    pub workspace_id: String,
    /// [`crate::ports::FireTarget::key`].
    pub target: String,
    /// The occurrence's listed time.
    pub scheduled_at: DateTime<Utc>,
}

/// A fire a worker holds the lease of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// Its row.
    pub key: FireKey,
    /// The attempt count this claim set: the row is this worker's while it holds it.
    pub token: i32,
    /// The schedule, when its stored spec can be read.
    pub spec: Option<ScheduleSpec>,
}

/// Why a done fire finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It recorded a run, with a session or with an error.
    Ran,
    /// Its following occurrence came due before it ran.
    Skipped,
    /// No handler owns its kind, or its spec cannot be read.
    Dropped,
}

impl Outcome {
    /// The `fire_claims.outcome` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ran => "ran",
            Self::Skipped => "skipped",
            Self::Dropped => "dropped",
        }
    }
}

/// What the claim of one fire delivered outside the queue allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// This caller owns the fire under `token`: run it, then finish it.
    Run {
        /// The claim's token.
        token: i32,
    },
    /// The fire ran, or ran out of attempts: drop the delivery.
    Done,
    /// Another replica owns the fire and its lease holds: deliver it again later.
    Held,
}

/// The fire queue, in durable storage.
pub trait FireClaims: Send + Sync {
    /// Claim the fire of `spec` at `at` outside the queue, or take over one whose lease ran out.
    fn take<'a>(
        &'a self,
        spec: &'a ScheduleSpec,
        at: DateTime<Utc>,
    ) -> BoxFuture<'a, Result<Claim, FireError>>;
    /// Add `fires`; an occurrence already queued keeps its row.
    fn enqueue<'a>(&'a self, fires: &'a [Fire]) -> BoxFuture<'a, Result<(), FireError>>;
    /// Claim up to `limit` due fires that are not done and whose lease is free.
    fn claim_due(&self, limit: usize) -> BoxFuture<'_, Result<Vec<Claimed>, FireError>>;
    /// Mark the fire done with `outcome`; `false` when another claim took it over.
    fn finish<'a>(&'a self, claim: &'a Claimed, outcome: Outcome) -> BoxFuture<'a, Result<bool, FireError>>;
    /// Free the fire to run again at `at`; `false` when another claim took it over.
    fn retry<'a>(&'a self, claim: &'a Claimed, at: DateTime<Utc>) -> BoxFuture<'a, Result<bool, FireError>>;
}

/// What an adapter implements to provide [`FireClaims`]; the methods are documented there.
pub trait NativeFireClaims: Send + Sync {
    fn take(
        &self,
        spec: &ScheduleSpec,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Claim, FireError>> + Send;
    fn enqueue(&self, fires: &[Fire]) -> impl Future<Output = Result<(), FireError>> + Send;
    fn claim_due(&self, limit: usize) -> impl Future<Output = Result<Vec<Claimed>, FireError>> + Send;
    fn finish(
        &self,
        claim: &Claimed,
        outcome: Outcome,
    ) -> impl Future<Output = Result<bool, FireError>> + Send;
    fn retry(
        &self,
        claim: &Claimed,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, FireError>> + Send;
}

impl<T: NativeFireClaims> FireClaims for T {
    fn take<'a>(
        &'a self,
        spec: &'a ScheduleSpec,
        at: DateTime<Utc>,
    ) -> BoxFuture<'a, Result<Claim, FireError>> {
        Box::pin(NativeFireClaims::take(self, spec, at))
    }
    fn enqueue<'a>(&'a self, fires: &'a [Fire]) -> BoxFuture<'a, Result<(), FireError>> {
        Box::pin(NativeFireClaims::enqueue(self, fires))
    }
    fn claim_due(&self, limit: usize) -> BoxFuture<'_, Result<Vec<Claimed>, FireError>> {
        Box::pin(NativeFireClaims::claim_due(self, limit))
    }
    fn finish<'a>(&'a self, claim: &'a Claimed, outcome: Outcome) -> BoxFuture<'a, Result<bool, FireError>> {
        Box::pin(NativeFireClaims::finish(self, claim, outcome))
    }
    fn retry<'a>(&'a self, claim: &'a Claimed, at: DateTime<Utc>) -> BoxFuture<'a, Result<bool, FireError>> {
        Box::pin(NativeFireClaims::retry(self, claim, at))
    }
}
