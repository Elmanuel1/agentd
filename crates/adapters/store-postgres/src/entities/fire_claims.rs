//! [`FireClaims`](shared::ports::fire::FireClaims) over `fire_claims`, the
//! fire queue: [`queue`] writes, claims and settles its rows; [`claim`] is the
//! claim a fire delivered outside the queue takes.

use chrono::{DateTime, Utc};
use shared::ports::ScheduleSpec;
use shared::ports::fire::{Claim, Claimed, Fire, FireError, NativeFireClaims, Outcome};

use crate::PgBackend;

mod claim;
pub(crate) mod queue;

/// The fire queue on Postgres.
#[derive(Debug, Clone)]
pub struct PgFireClaims {
    backend: PgBackend,
}

impl PgFireClaims {
    /// Over `backend`.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

fn unavailable(e: impl std::fmt::Display) -> FireError {
    FireError::Unavailable(e.to_string())
}

impl NativeFireClaims for PgFireClaims {
    async fn take(&self, spec: &ScheduleSpec, at: DateTime<Utc>) -> Result<Claim, FireError> {
        claim::take(&self.backend, spec, at).await
    }

    async fn enqueue(&self, fires: &[Fire]) -> Result<(), FireError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        queue::enqueue_in(&mut conn, fires).await.map_err(unavailable)
    }

    async fn claim_due(&self, limit: usize) -> Result<Vec<Claimed>, FireError> {
        queue::claim_due(&self.backend, limit).await
    }

    async fn finish(&self, claim: &Claimed, outcome: Outcome) -> Result<bool, FireError> {
        queue::finish(&self.backend, claim, outcome).await
    }

    async fn retry(&self, claim: &Claimed, at: DateTime<Utc>) -> Result<bool, FireError> {
        queue::retry(&self.backend, claim, at).await
    }
}
