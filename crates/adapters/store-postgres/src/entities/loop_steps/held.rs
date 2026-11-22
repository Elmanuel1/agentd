//! The writes of a held step: each lands only while its claim still holds
//! the step. Steps take no session-wide lock: a thread's step writes only
//! its events, and the session's row is locked only by a write that changes
//! it, the session's own step.

use ::sessions::steps::{Claim, StepEnd, StepWrite};
use ::sessions::store::StoreError;
use diesel::sql_types::{BigInt, Text};
use diesel_async::{AsyncConnection, RunQueryDsl};

use super::super::work_items::stop::requeue;
use super::land::land;
use super::{HELD, wake};
use crate::{PgBackend, diesel_be, pool_be};

/// `$3` the holder, `$4` the attempts and `$5` the thread, after the session's key.
fn held(
    sql: String,
    claim: &Claim,
) -> diesel::query_builder::BoxedSqlQuery<'_, diesel::pg::Pg, diesel::query_builder::SqlQuery> {
    diesel::sql_query(sql)
        .into_boxed()
        .bind::<Text, _>(claim.workspace.as_str())
        .bind::<Text, _>(claim.session.as_str())
        .bind::<Text, _>(&claim.owner)
        .bind::<BigInt, _>(claim.attempts)
        .bind::<Text, _>(claim.thread.as_ref().map_or("", |t| t.as_str()))
}

/// The transaction's result: `Ok(false)` when the claim no longer holds the
/// step, which rolls the transaction back.
fn lost(e: diesel::result::Error) -> Result<bool, StoreError> {
    use diesel::result::{DatabaseErrorKind, Error};
    match e {
        Error::RollbackTransaction => Ok(false),
        // A second checkpoint of one covered event (V11): another step wrote it.
        Error::DatabaseError(DatabaseErrorKind::UniqueViolation, info)
            if info.constraint_name() == Some("session_events_checkpoint") =>
        {
            Ok(false)
        }
        e => Err(diesel_be(e)),
    }
}

/// See `Steps::write`.
pub(super) async fn write(
    backend: &PgBackend,
    claim: &Claim,
    lease: i64,
    write: StepWrite<'_>,
) -> Result<bool, StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    conn.transaction::<_, diesel::result::Error, _>(async move |c| {
        let kept = held(
            format!("UPDATE loop_steps SET lease_until = now() + make_interval(secs => $6) WHERE {HELD}"),
            claim,
        )
        .bind::<BigInt, _>(lease)
        .execute(c)
        .await?;
        if kept == 0 {
            return Err(diesel::result::Error::RollbackTransaction);
        }
        land(c, claim, &write).await.map(|()| true)
    })
    .await
    .or_else(lost)
}

/// See `Steps::commit`.
pub(super) async fn commit(
    backend: &PgBackend,
    claim: &Claim,
    end: StepEnd,
    write: StepWrite<'_>,
) -> Result<bool, StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    conn.transaction::<_, diesel::result::Error, _>(async move |c| {
                // Rows are locked in one order, the session's own step first,
                // then threads' steps, then the session: a thread's step wakes
                // the session's step before it releases its own row, which
                // ends, and the session's step wakes threads after its own.
                if claim.thread.is_some() {
                    wake(c, &claim.workspace, &claim.session, &[""]).await?;
                }
                // Woken while it ran, or asked to go on: the step stays, due at once.
                let deleted = match end {
                    StepEnd::Again => 0,
                    StepEnd::Done | StepEnd::Requeue => {
                        held(format!("DELETE FROM loop_steps WHERE {HELD} AND wakes = $6"), claim)
                            .bind::<BigInt, _>(claim.wakes)
                            .execute(c)
                            .await?
                    }
                };
                let released = match deleted {
                    0 => held(
                        format!("UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, due_at = now() WHERE {HELD}"),
                        claim,
                    )
                    .execute(c)
                    .await?,
                    n => n,
                };
                if released == 0 {
                    return Err(diesel::result::Error::RollbackTransaction);
                }
                let woken: Vec<&str> = write.wake.iter().map(|t| t.as_str()).collect();
                wake(c, &claim.workspace, &claim.session, &woken).await?;
                land(c, claim, &write).await?;
                if end == StepEnd::Requeue {
                    requeue(c, &claim.workspace, claim.session.as_str()).await?;
                }
                Ok(true)
            })
        .await
        .or_else(lost)
}

/// See `Steps::defer`.
pub(super) async fn defer(backend: &PgBackend, claim: &Claim, seconds: i64) -> Result<(), StoreError> {
    let mut conn = backend.diesel.write().await.map_err(pool_be)?;
    held(
            format!(
                "UPDATE loop_steps SET lease_owner = NULL, lease_until = NULL, due_at = now() + make_interval(secs => $6) WHERE {HELD}"
            ),
            claim,
        )
        .bind::<BigInt, _>(seconds)
        .execute(&mut conn)
        .await
        .map(drop)
        .map_err(diesel_be)
}
