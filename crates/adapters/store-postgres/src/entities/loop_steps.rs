//! `Steps` on Postgres: the `loop_steps` queue, claimed with `FOR UPDATE
//! SKIP LOCKED` under a lease. Every write of a held step takes the
//! session's append lock first, as every append does, so the two never wait
//! on each other in a cycle.

use ::sessions::steps::{Claim, StepCounts, StepEnd, StepWrite, Steps};
use ::sessions::store::{StoreError, Wake};
use ::sessions::types::SessionId;
use async_trait::async_trait;
use diesel::sql_types::Text;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::WorkspaceId;

use crate::{diesel_be, pool_be, PgBackend};

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
/// `$3` the holder, `$4` the claim's attempts.
pub(super) const HELD: &str = "workspace_id = $1 AND session_id = $2 AND lease_owner = $3 AND attempts = $4";

mod claim;
mod held;
mod rows;

use self::rows::Counts;

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

#[async_trait]
impl Steps for PgSteps {
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
