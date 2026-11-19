//! [`FireClaims`](shared::ports::fire::FireClaims) over `fire_claims`, the
//! fire queue, whose rows [`queue`] writes, claims and settles and [`sweep`]
//! deletes.

use shared::ports::fire::FireError;

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
}

fn unavailable(e: impl std::fmt::Display) -> FireError {
    FireError::Unavailable(e.to_string())
}
