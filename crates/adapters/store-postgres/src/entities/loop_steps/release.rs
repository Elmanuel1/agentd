//! Releasing the steps a stopping worker still holds, in one statement: each
//! is due at once, unless another worker has taken it over since.

use ::sessions::steps::Claim;
use ::sessions::store::StoreError;
use diesel::sql_types::{Array, BigInt, Text};
use diesel_async::RunQueryDsl;

use crate::{PgBackend, diesel_be, pool_be};

pub(super) async fn release_all(
    backend: &PgBackend,
    owner: &str,
    claims: &[Claim],
) -> Result<u64, StoreError> {
    if claims.is_empty() {
        return Ok(0);
    }
    let workspaces: Vec<&str> = claims.iter().map(|c| c.workspace.as_str()).collect();
    let sessions: Vec<&str> = claims.iter().map(|c| c.session.as_str()).collect();
    let attempts: Vec<i64> = claims.iter().map(|c| c.attempts).collect();
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    let released = diesel::sql_query(
        "UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, due_at = now()
         FROM unnest($1::text[], $2::text[], $3::bigint[]) AS v(workspace_id, session_id, attempts)
         WHERE loop_steps.workspace_id = v.workspace_id AND loop_steps.session_id = v.session_id
           AND loop_steps.attempts = v.attempts AND loop_steps.lease_owner = $4",
    )
    .bind::<Array<Text>, _>(&workspaces)
    .bind::<Array<Text>, _>(&sessions)
    .bind::<Array<BigInt>, _>(&attempts)
    .bind::<Text, _>(owner)
    .execute(&mut conn)
    .await
    .map_err(diesel_be)?;
    Ok(u64::try_from(released).unwrap_or(0))
}
