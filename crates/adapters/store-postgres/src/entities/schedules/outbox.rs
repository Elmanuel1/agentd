//! The schedule outbox: each change is written in its deployment write's
//! transaction, and one replica at a time relays the rows, in order, to the
//! scheduler sink, removing each once it is applied.

use std::sync::Arc;
use std::time::Duration;

use diesel::sql_types::{Array, BigInt, Jsonb, Text};
use diesel::QueryableByName;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use futures::FutureExt;
use sessions::retention::RunLock;
use sessions::store::StoreError;
use shared::ports::{ScheduleChange, SchedulerPort};

use crate::{diesel_be, pool_be, PgBackend, PgRunLock};

/// Rows one relay pass reads.
const BATCH: i64 = 100;
/// The relay's lock across replicas.
const LOCK: &str = "schedule_outbox";

/// Record `change` in the transaction on `c`.
pub(crate) async fn record(
    c: &mut AsyncPgConnection,
    change: &ScheduleChange,
) -> Result<(), diesel::result::Error> {
    let doc =
        serde_json::to_value(change).map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    diesel::sql_query("INSERT INTO schedule_outbox (target, change) VALUES ($1, $2)")
        .bind::<Text, _>(change.target().key())
        .bind::<Jsonb, _>(doc)
        .execute(c)
        .await
        .map(drop)
}

#[derive(QueryableByName)]
struct Row {
    #[diesel(sql_type = BigInt)]
    seq: i64,
    #[diesel(sql_type = Jsonb)]
    change: serde_json::Value,
}

/// Relays the outbox to a scheduler sink.
#[derive(Clone)]
pub struct PgScheduleRelay {
    backend: PgBackend,
    lock: PgRunLock,
    sink: Arc<dyn SchedulerPort>,
}

impl std::fmt::Debug for PgScheduleRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgScheduleRelay").finish_non_exhaustive()
    }
}

impl PgScheduleRelay {
    /// Over `backend`, applying each change to `sink`.
    pub fn new(backend: PgBackend, sink: Arc<dyn SchedulerPort>) -> Self {
        Self { lock: PgRunLock::new(backend.clone()), backend, sink }
    }

    /// One pass, when this replica holds the relay's lock: apply the oldest
    /// rows in order, stop at the first the sink cannot take, and remove the
    /// applied ones. A change applied twice (a crash before its removal)
    /// leaves the same schedule.
    pub async fn relay(&self) -> Result<bool, StoreError> {
        self.lock.exclusive(LOCK, self.pass().boxed()).await
    }

    async fn pass(&self) -> Result<(), StoreError> {
        let mut conn = self.backend.diesel.write().await.map_err(pool_be)?;
        let rows: Vec<Row> =
            diesel::sql_query("SELECT seq, change FROM schedule_outbox ORDER BY seq LIMIT $1")
                .bind::<BigInt, _>(BATCH)
                .load(&mut conn)
                .await
                .map_err(diesel_be)?;
        let mut applied = Vec::new();
        for row in rows {
            let applies = match serde_json::from_value::<ScheduleChange>(row.change) {
                Ok(change) => change.apply(self.sink.as_ref()).await,
                Err(e) => {
                    tracing::error!(seq = row.seq, error = %e, "scheduler: an outbox row cannot be read; dropped");
                    Ok(())
                }
            };
            if let Err(e) = applies {
                tracing::warn!(seq = row.seq, error = %e, "scheduler: the sink refused a change; the next pass retries it");
                break;
            }
            applied.push(row.seq);
        }
        diesel::sql_query("DELETE FROM schedule_outbox WHERE seq = ANY($1)")
            .bind::<Array<BigInt>, _>(&applied)
            .execute(&mut conn)
            .await
            .map(drop)
            .map_err(diesel_be)
    }

    /// Relay every `interval`.
    pub fn spawn(self, interval: Duration) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = self.relay().await {
                    tracing::warn!(error = %e, "scheduler: the outbox relay failed");
                }
            }
        })
    }
}
