//! The fire inbox: every scheduler may deliver a fire more than once (a
//! retry, a redelivered stream entry), so each fire is claimed by its target
//! and time before it runs. One replica owns a fire at a time, under a lease;
//! a fire whose owner stopped is taken over once the lease runs out.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::FireError;
use crate::ports::ScheduleSpec;

/// What the claim of one fire allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// This caller owns the fire: run it, then finish it.
    Run,
    /// The fire ran, or ran out of attempts: drop the delivery.
    Done,
    /// Another replica owns the fire and its lease holds: deliver it again later.
    Held,
}

/// The fires claimed so far, in durable storage.
#[async_trait]
pub trait FireClaims: Send + Sync {
    /// Claim the fire of `spec` at `at`, or take over one whose lease ran out.
    async fn take(&self, spec: &ScheduleSpec, at: DateTime<Utc>) -> Result<Claim, FireError>;
    /// Record that the fire of `spec` at `at` ran.
    async fn finish(&self, spec: &ScheduleSpec, at: DateTime<Utc>) -> Result<(), FireError>;
}
