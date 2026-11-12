//! Schedules as durable timers (K-1, K-2): one row a target in `schedules`,
//! whose due rows any replica claims with `FOR UPDATE SKIP LOCKED`, so a
//! schedule registered on one replica fires on any, once.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use diesel::sql_types::{Array, Bool, Jsonb, Nullable, Text, Timestamptz};
use diesel::{OptionalExtension, QueryableByName};
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::ports::fire::FireRegistry;
use shared::ports::{FireTarget, ScheduleSpec, ScheduleState, SchedulerError, SchedulerPort};
use shared::schedule::{CompiledSchedule, CronKind, CronSchedule};

use crate::PgBackend;

pub(crate) mod outbox;

/// A fire later than this after its time is skipped, never sent late (no
/// backfill): after downtime the schedule moves on to its next time.
const MISFIRE: chrono::Duration = chrono::Duration::seconds(60);
/// Due rows one tick claims.
const BATCH: i64 = 100;

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
struct Due {
    #[diesel(sql_type = Text)]
    target: String,
    #[diesel(sql_type = Jsonb)]
    spec: serde_json::Value,
    #[diesel(sql_type = Timestamptz)]
    next_fire_at: DateTime<Utc>,
    #[diesel(sql_type = Timestamptz)]
    now: DateTime<Utc>,
}

#[derive(QueryableByName)]
struct Paused {
    #[diesel(sql_type = Bool)]
    paused: bool,
}

fn compile(spec: &ScheduleSpec) -> Result<CompiledSchedule, SchedulerError> {
    let cron =
        CronSchedule { kind: CronKind::Cron, expression: spec.cron.clone(), timezone: spec.timezone.clone() };
    cron.compile().map_err(|e| SchedulerError::InvalidSpec(e.to_string()))
}

fn unavailable(e: impl std::fmt::Display) -> SchedulerError {
    SchedulerError::Unavailable(e.to_string())
}

/// A due row's fire (its spec and time), when it is not too late, and its
/// next time, when it has one.
struct Advanced {
    fire: Option<(ScheduleSpec, DateTime<Utc>)>,
    next: Option<DateTime<Utc>>,
}

fn advance(due: &Due) -> Result<Advanced, SchedulerError> {
    let spec: ScheduleSpec = serde_json::from_value(due.spec.clone()).map_err(unavailable)?;
    let compiled = compile(&spec)?;
    let mut latest = due.next_fire_at;
    while let Some(n) = compiled.next_fires(latest, 1).into_iter().next().filter(|n| *n <= due.now) {
        latest = n;
    }
    let next = compiled.next_fires(due.now, 1).into_iter().next();
    let on_time = due.now - latest <= MISFIRE;
    Ok(Advanced { fire: on_time.then_some((spec, latest)), next })
}

impl PgScheduler {
    /// Over `backend`, delivering fires to `fires`.
    pub fn new(backend: PgBackend, fires: Arc<FireRegistry>) -> Self {
        Self { backend, fires }
    }

    /// Claim the due rows, move each to its next time (or remove one with
    /// none), then fire the on-time ones outside the transaction.
    pub async fn tick(&self) -> Result<(), SchedulerError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let fire = conn
            .transaction::<_, diesel::result::Error, _>(|c| {
                async move {
                    let due: Vec<Due> = diesel::sql_query(
                        "SELECT target, spec, next_fire_at, now() AS now FROM schedules
                         WHERE next_fire_at <= now() AND NOT paused
                         ORDER BY next_fire_at LIMIT $1 FOR UPDATE SKIP LOCKED",
                    )
                    .bind::<diesel::sql_types::BigInt, _>(BATCH)
                    .load(c)
                    .await?;
                    let (mut fire, mut targets, mut nexts) = (Vec::new(), Vec::new(), Vec::new());
                    for d in &due {
                        match advance(d) {
                            Ok(Advanced { fire: on_time, next }) => {
                                fire.extend(on_time);
                                targets.push(d.target.as_str());
                                nexts.push(next);
                            }
                            Err(e) => tracing::warn!(target = %d.target, error = %e, "scheduler: a schedule cannot be read"),
                        }
                    }
                    diesel::sql_query(
                        "UPDATE schedules SET next_fire_at = v.next FROM unnest($1::text[], $2::timestamptz[]) AS v(target, next)
                         WHERE schedules.target = v.target AND v.next IS NOT NULL",
                    )
                    .bind::<Array<Text>, _>(&targets)
                    .bind::<Array<Nullable<Timestamptz>>, _>(&nexts)
                    .execute(c)
                    .await?;
                    let ended: Vec<&str> =
                        targets.iter().zip(&nexts).filter(|(_, n)| n.is_none()).map(|(t, _)| *t).collect();
                    diesel::sql_query("DELETE FROM schedules WHERE target = ANY($1)")
                        .bind::<Array<Text>, _>(&ended)
                        .execute(c)
                        .await?;
                    Ok(fire)
                }
                .scope_boxed()
            })
            .await
            .map_err(unavailable)?;
        drop(conn);
        for (spec, at) in fire {
            if let Err(e) = self.fires.fire(&spec, at).await {
                tracing::warn!(target = %spec.target.key(), error = %e, "scheduler: fire dropped (no retry)");
            }
        }
        Ok(())
    }

    /// Tick every `interval`.
    pub fn spawn(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = self.tick().await {
                    tracing::warn!(error = %e, "scheduler: the tick failed");
                }
            }
        })
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
