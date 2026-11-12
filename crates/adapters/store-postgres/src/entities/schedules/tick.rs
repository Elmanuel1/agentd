//! The tick: claim the due rows, move each to its next time, fire the
//! on-time ones.

use std::time::Duration;

use chrono::{DateTime, Utc};
use diesel::sql_types::{Array, Jsonb, Nullable, Text, Timestamptz};
use diesel::QueryableByName;
use diesel_async::scoped_futures::ScopedFutureExt;
use diesel_async::{AsyncConnection, RunQueryDsl};
use shared::ports::{ScheduleSpec, SchedulerError};

use super::{compile, unavailable, PgScheduler};

/// A fire later than this after its time is skipped, never sent late (no
/// backfill): after downtime the schedule moves on to its next time.
const MISFIRE: chrono::Duration = chrono::Duration::seconds(60);
/// Due rows one tick claims.
const BATCH: i64 = 100;

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
}
