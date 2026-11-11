//! The writes of a held step: each lands only while its claim still holds
//! the step, under the session's append lock.

use ::sessions::steps::{Claim, StepEnd, StepWrite};
use ::sessions::store::StoreError;
use ::sessions::types::Session;
use diesel::sql_types::{BigInt, Jsonb, Text};
use diesel::OptionalExtension;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::Timestamp;

use super::super::session_events::{insert, lock, stamp};
use super::super::work_items::stop::requeue;
use super::rows::Doc;
use super::{HELD, KEY};
use crate::{diesel_be, enum_str, pool_be, to_doc, PgBackend};

/// The step's events and session changes, after the held step was checked.
async fn land(
    c: &mut AsyncPgConnection,
    claim: &Claim,
    write: &StepWrite<'_>,
) -> Result<(), diesel::result::Error> {
    let (ws, id) = (&claim.workspace, &claim.session);
    stamp(c, ws, id, write.read, write.at).await?;
    let ids: Vec<&str> = write.events.iter().map(|e| e.id.as_str()).collect();
    let docs = write.events.iter().map(to_doc).collect::<Result<Vec<_>, StoreError>>().map_err(serde_be)?;
    if insert(c, ws, id, &ids, docs).await? != ids.len() {
        return Err(diesel::result::Error::RollbackTransaction);
    }
    let row: Option<Doc> = diesel::sql_query(format!("SELECT doc FROM sessions WHERE {KEY} FOR UPDATE"))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .get_result(c)
        .await
        .optional()?;
    let Some(row) = row else {
        return Err(diesel::result::Error::RollbackTransaction);
    };
    let mut session: Session =
        serde_json::from_value(row.doc).map_err(|e| diesel::result::Error::DeserializationError(e.into()))?;
    if !(write.session)(&mut session) {
        return Ok(());
    }
    session.updated_at = Timestamp::now();
    let doc =
        serde_json::to_value(&session).map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    diesel::sql_query(format!("UPDATE sessions SET status = $3, doc = $4 WHERE {KEY}"))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>(enum_str(&session.status))
        .bind::<Jsonb, _>(doc)
        .execute(c)
        .await
        .map(drop)
}

fn serde_be(e: StoreError) -> diesel::result::Error {
    diesel::result::Error::SerializationError(Box::new(e))
}

/// `$3` the holder and `$4` the attempts, after the session's key.
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
}

/// The transaction's result: `Ok(false)` when the claim no longer holds the
/// step, which rolls the transaction back.
fn lost(e: diesel::result::Error) -> Result<bool, StoreError> {
    match e {
        diesel::result::Error::RollbackTransaction => Ok(false),
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
    conn.transaction::<_, diesel::result::Error, _>(|c| {
        async move {
            lock(c, &claim.session).await?;
            let kept = held(
                format!("UPDATE loop_steps SET lease_until = now() + make_interval(secs => $5) WHERE {HELD}"),
                claim,
            )
            .bind::<BigInt, _>(lease)
            .execute(c)
            .await?;
            if kept == 0 {
                return Err(diesel::result::Error::RollbackTransaction);
            }
            land(c, claim, &write).await.map(|()| true)
        }
        .scope_boxed()
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
    conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                lock(c, &claim.session).await?;
                // Woken while it ran, or asked to go on: the step stays, due at once.
                let deleted = match end {
                    StepEnd::Again => 0,
                    StepEnd::Done | StepEnd::Requeue => {
                        held(format!("DELETE FROM loop_steps WHERE {HELD} AND wakes = $5"), claim)
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
                land(c, claim, &write).await?;
                if end == StepEnd::Requeue {
                    requeue(c, &claim.workspace, claim.session.as_str()).await?;
                }
                Ok(true)
            }
            .scope_boxed()
        })
        .await
        .or_else(lost)
}

/// See `Steps::defer`.
pub(super) async fn defer(backend: &PgBackend, claim: &Claim, seconds: i64) -> Result<(), StoreError> {
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
