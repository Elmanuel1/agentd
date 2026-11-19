//! [`FireClaims`](shared::ports::fire::FireClaims) over `fire_claims`, the
//! fire queue, whose rows [`queue`] writes, claims and settles and [`sweep`]
//! deletes.

use chrono::{DateTime, Utc};
use shared::ports::fire::{Claimed, Fire, FireError, NativeFireClaims, Outcome};

use crate::PgBackend;

pub(crate) mod queue;
mod sweep;

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

    /// The fires due and not done.
    pub async fn count_due(&self) -> Result<i64, FireError> {
        queue::count_due(&self.backend).await
    }
}

fn unavailable(e: impl std::fmt::Display) -> FireError {
    FireError::Unavailable(e.to_string())
}

impl NativeFireClaims for PgFireClaims {
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
