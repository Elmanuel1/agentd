//! Releasing a held step unrun: deferred by a set time (an over-budget step,
//! an MCP refresh), or after a failure, by a random wait that doubles with
//! the step's failures in a row: up to 5 s after the first, at most 5
//! minutes (full jitter, SH-9).

use ::sessions::steps::Claim;
use ::sessions::store::StoreError;
use diesel::sql_types::BigInt;
use diesel_async::RunQueryDsl;

use super::super::HELD;
use super::held;
use crate::{PgBackend, diesel_be, pool_be};

/// See `Steps::defer`.
pub(in super::super) async fn defer(
    backend: &PgBackend,
    claim: &Claim,
    seconds: i64,
) -> Result<(), StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    held(
        format!(
            "UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, due_at = now() + make_interval(secs => $5) WHERE {HELD}"
        ),
        claim,
    )
    .bind::<BigInt, _>(seconds)
    .execute(&mut conn)
    .await
    .map(drop)
    .map_err(diesel_be)
}

/// See `Steps::defer_failed`.
pub(in super::super) async fn defer_failed(backend: &PgBackend, claim: &Claim) -> Result<(), StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    // `SET` reads the old row, so the wait counts the failures before this one.
    held(
        format!(
            "UPDATE loop_steps SET failures = failures + 1, lease_owner = NULL, lease_until = NULL,
               due_at = now() + random() * least(300, 5 * 2 ^ least(failures, 6)) * interval '1 second'
             WHERE {HELD}"
        ),
        claim,
    )
    .execute(&mut conn)
    .await
    .map(drop)
    .map_err(diesel_be)
}
