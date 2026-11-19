//! The claim of a fire delivered outside the queue: the insert is the claim,
//! so of the replicas a fire reaches at once exactly one owns it. A claim not
//! done when its lease runs out is taken over in the same statement, up to
//! [`ATTEMPTS`] times.

use chrono::{DateTime, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Bool, Double, Integer, Text, Timestamptz};
use diesel_async::RunQueryDsl;
use shared::ports::ScheduleSpec;
use shared::ports::fire::{Claim, FireError};

use super::unavailable;
use crate::PgBackend;

/// How long one owner holds a fire; longer than any fire runs.
const LEASE_SECS: f64 = 900.0;
/// Runs of one fire before it is dropped.
const ATTEMPTS: i32 = 3;

#[derive(QueryableByName)]
struct Won {
    #[diesel(sql_type = Integer)]
    attempts: i32,
}

#[derive(QueryableByName)]
struct Standing {
    #[diesel(sql_type = Bool)]
    done: bool,
    #[diesel(sql_type = Bool)]
    spent: bool,
}

pub(super) async fn take(
    backend: &PgBackend,
    spec: &ScheduleSpec,
    at: DateTime<Utc>,
) -> Result<Claim, FireError> {
    let mut conn = backend.diesel.write().await.map_err(unavailable)?;
    let won: Vec<Won> = diesel::sql_query(
        "INSERT INTO fire_claims (workspace_id, target, scheduled_at) VALUES ($1, $2, $3)
         ON CONFLICT (workspace_id, target, scheduled_at) DO UPDATE
           SET claimed_at = now(), attempts = fire_claims.attempts + 1
           WHERE NOT fire_claims.done AND fire_claims.attempts < $4
             AND fire_claims.claimed_at < now() - make_interval(secs => $5)
         RETURNING attempts",
    )
    .bind::<Text, _>(&spec.tenant.workspace_id)
    .bind::<Text, _>(spec.target.key())
    .bind::<Timestamptz, _>(at)
    .bind::<Integer, _>(ATTEMPTS)
    .bind::<Double, _>(LEASE_SECS)
    .load(&mut conn)
    .await
    .map_err(unavailable)?;
    if let Some(won) = won.first() {
        return Ok(Claim::Run { token: won.attempts });
    }
    let standing: Standing = diesel::sql_query(
        "SELECT done, attempts >= $4 AND claimed_at < now() - make_interval(secs => $5) AS spent
         FROM fire_claims WHERE workspace_id = $1 AND target = $2 AND scheduled_at = $3",
    )
    .bind::<Text, _>(&spec.tenant.workspace_id)
    .bind::<Text, _>(spec.target.key())
    .bind::<Timestamptz, _>(at)
    .bind::<Integer, _>(ATTEMPTS)
    .bind::<Double, _>(LEASE_SECS)
    .get_result(&mut conn)
    .await
    .map_err(unavailable)?;
    match (standing.done, standing.spent) {
        (true, _) => Ok(Claim::Done),
        (false, true) => {
            tracing::error!(target = %spec.target.key(), %at, attempts = ATTEMPTS, "fires: a fire failed every attempt and is dropped");
            Ok(Claim::Done)
        }
        (false, false) => Ok(Claim::Held),
    }
}
