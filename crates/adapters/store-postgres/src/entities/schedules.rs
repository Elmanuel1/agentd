//! Schedules as durable timers (K-1, K-2): one row a target in `schedules`,
//! whose due rows any replica claims with `FOR UPDATE SKIP LOCKED`, so a
//! schedule registered on one replica fires on any, once.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use diesel::sql_types::{Bool, Jsonb, Text, Timestamptz};
use diesel::{OptionalExtension, QueryableByName};
use diesel_async::RunQueryDsl;
use shared::ports::fire::FireRegistry;
use shared::ports::{FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort};
use shared::schedule::CompiledSchedule;

use crate::PgBackend;

pub mod outbox;
mod tick;

/// [`SchedulerPort`] over Postgres; [`PgScheduler::spawn`] ticks it.
#[derive(Clone)]
pub struct PgScheduler {
    backend: PgBackend,
    fires: Arc<FireRegistry>,
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

fn compile(spec: &ScheduleSpec) -> Result<CompiledSchedule, SchedulerError> {
    spec.compile().map_err(|e| SchedulerError::InvalidSpec(e.to_string()))
}

fn unavailable(e: impl std::fmt::Display) -> SchedulerError {
    SchedulerError::Unavailable(e.to_string())
}

impl PgScheduler {
    /// Over `backend`, delivering fires to `fires`.
    pub fn new(backend: PgBackend, fires: Arc<FireRegistry>) -> Self {
        Self { backend, fires }
    }

    async fn set_paused(&self, target: &FireTarget, paused: bool) -> Result<(), SchedulerError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let changed = diesel::sql_query("UPDATE schedules SET paused = $2 WHERE target = $1")
            .bind::<Text, _>(target.key())
            .bind::<Bool, _>(paused)
            .execute(&mut conn)
            .await
            .map_err(unavailable)?;
        match changed {
            0 => Err(SchedulerError::Unknown(target.key())),
            _ => Ok(()),
        }
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
        self.set_paused(target, true).await
    }

    async fn unpause(&self, target: &FireTarget) -> Result<(), SchedulerError> {
        self.set_paused(target, false).await
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
