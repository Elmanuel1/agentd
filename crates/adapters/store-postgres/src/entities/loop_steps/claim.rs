//! Claiming a due step under a lease, within its workspace's limit.

use ::sessions::steps::Claim;
use ::sessions::store::StoreError;
use ::sessions::types::SessionId;
use diesel::sql_types::{BigInt, Text};
use diesel::OptionalExtension;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::WorkspaceId;

use super::rows::{Counts, Due, Flag, Held};
use super::KEY;
use crate::{diesel_be, pool_be, PgBackend};

/// The oldest due step, held by `owner`; see `Steps::claim`.
pub(super) async fn claim(
    backend: &PgBackend,
    owner: &str,
    lease: i64,
    per_workspace: i64,
) -> Result<Option<Claim>, StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                let due: Option<Due> = diesel::sql_query(
                    "SELECT workspace_id, session_id FROM loop_steps
                     WHERE due_at <= now() AND (lease_until IS NULL OR lease_until < now())
                     ORDER BY due_at LIMIT 1 FOR UPDATE SKIP LOCKED",
                )
                .get_result(c)
                .await
                .optional()?;
                let Some(due) = due else { return Ok(None) };
                // A claimer never waits on the workspace lock while it holds a row.
                let free: Flag = diesel::sql_query("SELECT pg_try_advisory_xact_lock(hashtext($1)) AS held")
                    .bind::<Text, _>(&due.workspace_id)
                    .get_result(c)
                    .await?;
                if !free.held {
                    return Ok(None);
                }
                let busy: Counts = diesel::sql_query(
                    "SELECT count(*) AS leased, 0::bigint AS due FROM loop_steps WHERE workspace_id = $1 AND lease_until > now()",
                )
                .bind::<Text, _>(&due.workspace_id)
                .get_result(c)
                .await?;
                if busy.leased >= per_workspace {
                    diesel::sql_query(
                        "UPDATE loop_steps SET due_at = now() + interval '1 second' WHERE workspace_id = $1 AND lease_until IS NULL",
                    )
                    .bind::<Text, _>(&due.workspace_id)
                    .execute(c)
                    .await?;
                    return Ok(None);
                }
                let held: Held = diesel::sql_query(format!(
                    "UPDATE loop_steps SET lease_owner = $3, lease_until = now() + make_interval(secs => $4),
                     attempts = attempts + 1 WHERE {KEY} RETURNING attempts, wakes"
                ))
                .bind::<Text, _>(&due.workspace_id)
                .bind::<Text, _>(&due.session_id)
                .bind::<Text, _>(owner)
                .bind::<BigInt, _>(lease)
                .get_result(c)
                .await?;
                let session = SessionId::parse(&due.session_id).map_err(|e| diesel::result::Error::DeserializationError(e.into()))?;
                Ok(Some(Claim {
                    workspace: WorkspaceId::new(due.workspace_id),
                    session,
                    owner: owner.to_owned(),
                    attempts: held.attempts,
                    wakes: held.wakes,
                }))
            }
            .scope_boxed()
        })
        .await
        .map_err(diesel_be)
}
