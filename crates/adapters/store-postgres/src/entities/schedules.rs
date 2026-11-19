//! Schedules as durable timers (K-1, K-2): one row a target in `schedules`,
//! whose due rows any replica claims with `FOR UPDATE SKIP LOCKED`, so a
//! schedule registered on one replica fires on any, once.

use async_trait::async_trait;
use chrono::Utc;
use diesel::sql_types::{Bool, Jsonb, Text, Timestamptz};
use diesel::{OptionalExtension, QueryableByName};
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::ports::{FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort};
use shared::schedule::CompiledSchedule;

use crate::PgBackend;

pub mod outbox;
mod tick;

/// [`SchedulerPort`] over Postgres; [`PgScheduler::spawn`] ticks it.
#[derive(Clone)]
pub struct PgScheduler {
    backend: PgBackend,
}

impl std::fmt::Debug for PgScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgScheduler").finish_non_exhaustive()
    }
}

#[derive(QueryableByName)]
struct Paused {
    #[diesel(sql_type = Bool)]
    paused: bool,
}

#[derive(QueryableByName)]
struct Stored {
    #[diesel(sql_type = Jsonb)]
    spec: serde_json::Value,
}

fn compile(spec: &ScheduleSpec) -> Result<CompiledSchedule, SchedulerError> {
    spec.compile().map_err(|e| SchedulerError::InvalidSpec(e.to_string()))
}

fn unavailable(e: impl std::fmt::Display) -> SchedulerError {
    SchedulerError::Unavailable(e.to_string())
}

impl PgScheduler {
    /// Over `backend`; its occurrences go to the fire queue.
    pub fn new(backend: PgBackend) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl SchedulerPort for PgScheduler {
    async fn upsert(&self, spec: ScheduleSpec) -> Result<(), SchedulerError> {
        let next = compile(&spec)?.next_fires(Utc::now(), 1).into_iter().next();
        let next = next.ok_or_else(|| SchedulerError::InvalidSpec("the schedule never fires".into()))?;
        let doc = serde_json::to_value(&spec).map_err(unavailable)?;
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        diesel::sql_query(
            "INSERT INTO schedules (target, workspace_id, spec, next_fire_at) VALUES ($1, $2, $3, $4)
             ON CONFLICT (target) DO UPDATE SET workspace_id = EXCLUDED.workspace_id, spec = EXCLUDED.spec,
               next_fire_at = EXCLUDED.next_fire_at, paused = false",
        )
        .bind::<Text, _>(spec.target.key())
        .bind::<Text, _>(&spec.tenant.workspace_id)
        .bind::<Jsonb, _>(doc)
        .bind::<Timestamptz, _>(next)
        .execute(&mut conn)
        .await
        .map(drop)
        .map_err(unavailable)
    }

    async fn pause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let changed = diesel::sql_query("UPDATE schedules SET paused = true WHERE target = $1")
            .bind::<Text, _>(target.key())
            .execute(&mut conn)
            .await
            .map_err(unavailable)?;
        match changed {
            0 => Err(SchedulerError::Unknown(target.key())),
            _ => Ok(()),
        }
    }

    /// Resume from the next occurrence after now, worked out once the row is locked, so a write that
    /// waited cannot resume at a time missed while paused.
    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let key = target.key();
        conn.transaction::<_, diesel::result::Error, _>(async move |c| {
            let row: Option<Stored> =
                diesel::sql_query("SELECT spec FROM schedules WHERE target = $1 FOR UPDATE")
                    .bind::<Text, _>(&key)
                    .get_result(c)
                    .await
                    .optional()?;
            let Some(row) = row else { return Ok(Err(SchedulerError::Unknown(key))) };
            let spec: Result<ScheduleSpec, _> = serde_json::from_value(row.spec).map_err(unavailable);
            let next =
                spec.and_then(|spec| compile(&spec)).map(|c| c.next_fires(Utc::now(), 1).into_iter().next());
            let next = match next {
                Ok(Some(next)) => next,
                Ok(None) => return Ok(Err(SchedulerError::InvalidSpec("the schedule never fires".into()))),
                Err(e) => return Ok(Err(e)),
            };
            diesel::sql_query("UPDATE schedules SET paused = false, next_fire_at = $2 WHERE target = $1")
                .bind::<Text, _>(&key)
                .bind::<Timestamptz, _>(next)
                .execute(c)
                .await?;
            Ok(Ok(()))
        })
        .await
        .map_err(unavailable)?
    }

    async fn delete(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let gone = diesel::sql_query("DELETE FROM schedules WHERE target = $1")
            .bind::<Text, _>(target.key())
            .execute(&mut conn)
            .await
            .map_err(unavailable)?;
        match gone {
            0 => Err(SchedulerError::Unknown(target.key())),
            _ => Ok(()),
        }
    }

    async fn describe(&self, target: &FireTarget) -> Result<ScheduleState, SchedulerError> {
        let mut conn = self.backend.diesel.get().await.map_err(unavailable)?;
        let row: Option<Paused> = diesel::sql_query("SELECT paused FROM schedules WHERE target = $1")
            .bind::<Text, _>(target.key())
            .get_result(&mut conn)
            .await
            .optional()
            .map_err(unavailable)?;
        row.map(|r| ScheduleState { paused: r.paused }).ok_or_else(|| SchedulerError::Unknown(target.key()))
    }
}
