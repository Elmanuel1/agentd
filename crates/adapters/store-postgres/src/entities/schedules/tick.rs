//! The tick: in batches, claim the due rows, write each one's latest due
//! occurrence to the fire queue and move it to its next time, in the same
//! transaction, so an occurrence is never lost once its schedule moves on.

use std::time::Duration;

use chrono::{DateTime, Utc};
use diesel::QueryableByName;
use diesel::sql_types::{Array, BigInt, Jsonb, Nullable, Text, Timestamptz};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use shared::ports::fire::{Fire, jitter};
use shared::ports::{ScheduleSpec, SchedulerError};

use super::{PgScheduler, compile, unavailable};
use crate::entities::fire_claims::queue;

/// Due rows one batch claims.
const BATCH: usize = 100;

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

/// A due row's latest occurrence at or before now, older ones dropped, and
/// its next time, when it has one.
fn advance(due: &Due) -> Result<(Fire, Option<DateTime<Utc>>), SchedulerError> {
    let spec: ScheduleSpec = serde_json::from_value(due.spec.clone()).map_err(unavailable)?;
    let compiled = compile(&spec)?;
    let mut latest = due.next_fire_at;
    while let Some(n) = compiled.next_fires(latest, 1).into_iter().next().filter(|n| *n <= due.now) {
        latest = n;
    }
    let next = compiled.next_fires(due.now, 1).into_iter().next();
    let due_at = jitter::due_at(&spec, latest);
    Ok((Fire { spec, scheduled_at: latest, due_at }, next))
}

impl PgScheduler {
    /// Write every due schedule's occurrence to the fire queue, a batch per
    /// transaction until a batch comes back short; how many it wrote.
    pub async fn tick(&self) -> Result<usize, SchedulerError> {
        let mut conn = self.backend.diesel.write().await.map_err(unavailable)?;
        let mut written = 0;
        loop {
            let (claimed, fires) = conn.transaction(async |c| batch(c).await).await.map_err(unavailable)?;
            written += fires;
            if claimed < BATCH {
                return Ok(written);
            }
        }
    }

    /// Tick every `interval`; `enqueued` hears how many occurrences each tick wrote.
    pub fn spawn(
        self,
        interval: Duration,
        enqueued: impl Fn(usize) + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                match self.tick().await {
                    Ok(written) => enqueued(written),
                    Err(e) => tracing::warn!(error = %e, "scheduler: the tick failed"),
                }
            }
        })
    }
}

/// One batch: the rows it claimed, and the occurrences it wrote.
async fn batch(c: &mut AsyncPgConnection) -> Result<(usize, usize), diesel::result::Error> {
    let due: Vec<Due> = diesel::sql_query(
        "SELECT target, spec, next_fire_at, now() AS now FROM schedules
         WHERE next_fire_at <= now() AND NOT paused
         ORDER BY next_fire_at LIMIT $1 FOR UPDATE SKIP LOCKED",
    )
    .bind::<BigInt, _>(i64::try_from(BATCH).unwrap_or(i64::MAX))
    .load(c)
    .await?;
    let (mut fires, mut targets, mut nexts, mut unreadable) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for d in &due {
        match advance(d) {
            Ok((fire, next)) => {
                fires.push(fire);
                targets.push(d.target.as_str());
                nexts.push(next);
            }
            Err(e) => {
                tracing::error!(target = %d.target, error = %e, "scheduler: a schedule cannot be read; paused");
                unreadable.push(d.target.as_str());
            }
        }
    }
    queue::enqueue_in(c, &fires).await?;
    diesel::sql_query(
        "UPDATE schedules SET next_fire_at = v.next FROM unnest($1::text[], $2::timestamptz[]) AS v(target, next)
         WHERE schedules.target = v.target AND v.next IS NOT NULL",
    )
    .bind::<Array<Text>, _>(&targets)
    .bind::<Array<Nullable<Timestamptz>>, _>(&nexts)
    .execute(c)
    .await?;
    diesel::sql_query("UPDATE schedules SET paused = true WHERE target = ANY($1)")
        .bind::<Array<Text>, _>(&unreadable)
        .execute(c)
        .await?;
    let ended: Vec<&str> = targets.iter().zip(&nexts).filter(|(_, n)| n.is_none()).map(|(t, _)| *t).collect();
    diesel::sql_query("DELETE FROM schedules WHERE target = ANY($1)")
        .bind::<Array<Text>, _>(&ended)
        .execute(c)
        .await?;
    Ok((due.len(), fires.len()))
}
