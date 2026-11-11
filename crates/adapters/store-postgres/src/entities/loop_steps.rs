//! `Steps` on Postgres: the `loop_steps` queue, claimed with `FOR UPDATE
//! SKIP LOCKED` under a lease. Every write of a held step takes the
//! session's append lock first, as every append does, so the two never wait
//! on each other in a cycle.

use ::sessions::steps::{Claim, StepCounts, StepEnd, StepWrite, Steps};
use ::sessions::store::{StoreError, Wake};
use ::sessions::types::{Session, SessionId};
use async_trait::async_trait;
use diesel::sql_types::{BigInt, Bool, Jsonb, Text};
use diesel::OptionalExtension;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::{Timestamp, WorkspaceId};

use super::session_events::{insert, lock, stamp};
use crate::{diesel_be, enum_str, pool_be, to_doc, PgBackend};

/// Postgres-backed step queue.
#[derive(Debug, Clone)]
pub struct PgSteps {
    backend: PgBackend,
}

impl PgSteps {
    /// Wrap a backend.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

const KEY: &str = "workspace_id = $1 AND session_id = $2";
/// `$3` the holder, `$4` the claim's attempts.
const HELD: &str = "workspace_id = $1 AND session_id = $2 AND lease_owner = $3 AND attempts = $4";

#[derive(diesel::QueryableByName)]
struct Due {
    #[diesel(sql_type = Text)]
    workspace_id: String,
    #[diesel(sql_type = Text)]
    session_id: String,
}

#[derive(diesel::QueryableByName)]
struct Held {
    #[diesel(sql_type = BigInt)]
    attempts: i64,
    #[diesel(sql_type = BigInt)]
    wakes: i64,
}

#[derive(diesel::QueryableByName)]
struct Flag {
    #[diesel(sql_type = Bool)]
    held: bool,
}

#[derive(diesel::QueryableByName)]
struct Counts {
    #[diesel(sql_type = BigInt)]
    due: i64,
    #[diesel(sql_type = BigInt)]
    leased: i64,
}

#[derive(diesel::QueryableByName)]
struct Doc {
    #[diesel(sql_type = Jsonb)]
    doc: serde_json::Value,
}

/// Put the session on the queue in the append's transaction, or wake the
/// step it already has.
pub(crate) async fn enqueue(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    wake: Wake,
) -> Result<(), diesel::result::Error> {
    let sql = match wake {
        Wake::No => return Ok(()),
        Wake::Step => {
            "INSERT INTO loop_steps (workspace_id, session_id) VALUES ($1, $2) ON CONFLICT (workspace_id, session_id) \
             DO UPDATE SET wakes = loop_steps.wakes + 1, due_at = least(loop_steps.due_at, now())"
        }
        Wake::IfQueued => {
            "UPDATE loop_steps SET wakes = wakes + 1, due_at = least(due_at, now()) \
             WHERE workspace_id = $1 AND session_id = $2"
        }
    };
    diesel::sql_query(sql)
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .execute(c)
        .await
        .map(drop)
}

/// The session's step goes with the session, in its delete's transaction.
pub(crate) async fn forget(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
) -> Result<(), diesel::result::Error> {
    diesel::sql_query(format!("DELETE FROM loop_steps WHERE {KEY}"))
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .execute(c)
        .await
        .map(drop)
}

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

#[async_trait]
impl Steps for PgSteps {
    async fn claim(&self, owner: &str, lease: i64, per_workspace: i64) -> Result<Option<Claim>, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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

    async fn write(&self, claim: &Claim, lease: i64, write: StepWrite<'_>) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        conn.transaction::<_, diesel::result::Error, _>(|c| {
            async move {
                lock(c, &claim.session).await?;
                let kept = held(
                    format!(
                        "UPDATE loop_steps SET lease_until = now() + make_interval(secs => $5) WHERE {HELD}"
                    ),
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

    async fn commit(&self, claim: &Claim, end: StepEnd, write: StepWrite<'_>) -> Result<bool, StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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
                    requeue(c, &claim.workspace, &claim.session).await?;
                }
                Ok(true)
            }
            .scope_boxed()
        })
        .await
        .or_else(lost)
    }

    async fn defer(&self, claim: &Claim, seconds: i64) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
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

    async fn counts(&self) -> Result<StepCounts, StoreError> {
        let mut conn = self.backend.diesel.get().await.map_err(pool_be)?;
        // A lease that ran out is claimable again, so it counts as due.
        let c: Counts = diesel::sql_query(
            "SELECT count(*) FILTER (WHERE due_at <= now() AND (lease_until IS NULL OR lease_until < now())) AS due,
                    count(*) FILTER (WHERE lease_until > now()) AS leased FROM loop_steps",
        )
        .get_result(&mut conn)
        .await
        .map_err(diesel_be)?;
        Ok(StepCounts { due: c.due, leased: c.leased })
    }
}

/// The session's stopped work item, queued again on its own queue for a
/// worker to run the calls the step parked.
async fn requeue(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
) -> Result<(), diesel::result::Error> {
    diesel::sql_query(format!(
        "UPDATE work_items SET state = 'queued', stop_requested_at = NULL, stopped_at = NULL, acknowledged_at = NULL,
         handed_out_at = NULL, lease_until = NULL, latest_heartbeat_at = NULL, last_heartbeat = NULL, started_at = now()
         WHERE {KEY} AND (state = 'stopped' OR (state = 'stopping' AND lease_until < now()))"
    ))
    .bind::<Text, _>(ws.as_str())
    .bind::<Text, _>(id.as_str())
    .execute(c)
    .await
    .map(drop)
}
