//! `Steps` on Postgres: the `loop_steps` queue, claimed with `FOR UPDATE
//! SKIP LOCKED` under a lease. Every write of a held step takes the
//! session's append lock first, as every append does, so the two never wait
//! on each other in a cycle.

use ::sessions::steps::{Claim, NativeSteps, StepCounts, StepEnd, StepWrite};
use ::sessions::store::{StoreError, Wake};
use ::sessions::types::SessionId;
use diesel::sql_types::Text;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::WorkspaceId;

use crate::{PgBackend, diesel_be, pool_be};

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

pub(super) const KEY: &str = "workspace_id = $1 AND session_id = $2";
/// `$3` the holder, `$4` the claim's attempts, `$5` the thread (`''` for the
/// session's own step).
pub(super) const HELD: &str =
    "workspace_id = $1 AND session_id = $2 AND lease_owner = $3 AND attempts = $4 AND thread_id = $5";

mod claim;
mod held;
mod rows;

use self::rows::Counts;

/// Put the session's own step on the queue in the append's transaction.
pub(crate) async fn enqueue(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    wake: Wake,
) -> Result<(), diesel::result::Error> {
    let sql = match wake {
        Wake::No => return Ok(()),
        Wake::Step => WAKE,
    };
    diesel::sql_query(sql)
        .bind::<Text, _>(ws.as_str())
        .bind::<Text, _>(id.as_str())
        .bind::<Text, _>("")
        .execute(c)
        .await
        .map(drop)
}

/// A step due at once for `thread` (`''`: the session's own), or a wake of
/// the one it has.
const WAKE: &str = "INSERT INTO loop_steps (workspace_id, session_id, thread_id) VALUES ($1, $2, $3) \
     ON CONFLICT (workspace_id, session_id, thread_id) \
     DO UPDATE SET wakes = loop_steps.wakes + 1, due_at = least(loop_steps.due_at, now())";

/// Give each of `threads` (`''`: the session's own step) a step due at once.
pub(super) async fn wake(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
    threads: &[&str],
) -> Result<(), diesel::result::Error> {
    for thread in threads {
        diesel::sql_query(WAKE)
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .bind::<Text, _>(*thread)
            .execute(c)
            .await?;
    }
    Ok(())
}

/// The session's steps go with the session, in its delete's transaction,
/// before its row: the session's own step first, in the order a step's
/// commit locks them, so the two never wait on each other in a cycle.
pub(crate) async fn forget(
    c: &mut AsyncPgConnection,
    ws: &WorkspaceId,
    id: &SessionId,
) -> Result<(), diesel::result::Error> {
    for scope in ["AND thread_id = ''", ""] {
        diesel::sql_query(format!("DELETE FROM loop_steps WHERE {KEY} {scope}"))
            .bind::<Text, _>(ws.as_str())
            .bind::<Text, _>(id.as_str())
            .execute(c)
            .await?;
    }
    Ok(())
}

impl NativeSteps for PgSteps {
    async fn claim(&self, owner: &str, lease: i64, per_workspace: i64) -> Result<Option<Claim>, StoreError> {
        claim::claim(&self.backend, owner, lease, per_workspace).await
    }

    async fn write(&self, claim: &Claim, lease: i64, write: StepWrite<'_>) -> Result<bool, StoreError> {
        held::write(&self.backend, claim, lease, write).await
    }

    async fn commit(&self, claim: &Claim, end: StepEnd, write: StepWrite<'_>) -> Result<bool, StoreError> {
        held::commit(&self.backend, claim, end, write).await
    }

    async fn defer(&self, claim: &Claim, seconds: i64) -> Result<(), StoreError> {
        held::defer(&self.backend, claim, seconds).await
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
