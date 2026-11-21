//! Releasing a held step unrun: deferred by a set time (an over-budget step,
//! an MCP refresh), or after a failure, by a random wait that doubles with
//! the step's failures in a row: up to 5 s after the first, at most 5
//! minutes (full jitter, SH-9).

use ::sessions::steps::{Claim, Wait};
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
    wait: Wait,
) -> Result<(), StoreError> {
    // `SET` reads the old row, so a failed step's wait counts the failures before this one.
    let due = match wait {
        Wait::Seconds(_) => "due_at = now() + make_interval(secs => $5)",
        Wait::Failed => {
            "failures = failures + 1, due_at = now() + random() * least(300, 5 * 2 ^ least(failures, 6)) * interval '1 second'"
        }
    };
    let query = held(
        format!("UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, {due} WHERE {HELD}"),
        claim,
    );
    let query = match wait {
        Wait::Seconds(seconds) => query.bind::<BigInt, _>(seconds),
        Wait::Failed => query,
    };
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    query.execute(&mut conn).await.map(drop).map_err(diesel_be)
}
