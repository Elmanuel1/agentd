//! The schedule outbox: each change is inserted and deleted again in its
//! deployment write's transaction, so the table stays empty while the WAL
//! keeps the insert. The change feed hands each insert, in commit order, to
//! [`ScheduleOutbox`], which applies it to the scheduler sink.

use shared::BackendError;
use std::sync::Arc;

use diesel::QueryableByName;
use diesel::sql_types::{Array, BigInt, Jsonb, Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::ports::changes::{Change, ChangeOp, NativeChangeHandler, Source};
use shared::ports::{ScheduleChange, SchedulerError, SchedulerPort};

/// Record `changes`, in order, in the transaction on `c`.
pub(crate) async fn record(
    c: &mut AsyncPgConnection,
    changes: &[ScheduleChange],
) -> Result<(), diesel::result::Error> {
    if changes.is_empty() {
        return Ok(());
    }
    let targets: Vec<String> = changes.iter().map(|change| change.target().key()).collect();
    let docs = changes
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    // Two statements: a DELETE in the INSERT's own statement cannot see the rows.
    let added: Vec<Added> = diesel::sql_query(
        "INSERT INTO schedule_outbox (target, change)
         SELECT target, change FROM unnest($1::text[], $2::jsonb[]) AS c(target, change) RETURNING seq",
    )
    .bind::<Array<Text>, _>(targets)
    .bind::<Array<Jsonb>, _>(docs)
    .load(c)
    .await?;
    let seqs: Vec<i64> = added.into_iter().map(|a| a.seq).collect();
    diesel::sql_query("DELETE FROM schedule_outbox WHERE seq = ANY($1)")
        .bind::<Array<BigInt>, _>(seqs)
        .execute(c)
        .await
        .map(drop)
}

#[derive(QueryableByName)]
struct Added {
    #[diesel(sql_type = BigInt)]
    seq: i64,
}

/// Applies the outbox's changes to a scheduler sink, one at a time, in order.
#[derive(Clone)]
pub struct ScheduleOutbox {
    sink: Arc<dyn SchedulerPort>,
}

impl std::fmt::Debug for ScheduleOutbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduleOutbox").finish_non_exhaustive()
    }
}

impl ScheduleOutbox {
    /// Applying each change to `sink`.
    pub fn new(sink: Arc<dyn SchedulerPort>) -> Self {
        Self { sink }
    }
}

impl NativeChangeHandler for ScheduleOutbox {
    fn source(&self) -> Source {
        Source::Table("schedule_outbox".into())
    }

    fn ordered(&self) -> bool {
        true
    }

    /// An insert's change, applied again after a crash, leaves the same
    /// schedule; its delete in the same transaction is not a change to apply.
    /// A change that never decodes, or that the sink refuses as invalid, is
    /// passed over, so it cannot hold back the ones after it; a sink that is
    /// unavailable is retried.
    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let Some(row) = change.after.as_ref().filter(|_| change.op == ChangeOp::Insert) else {
            return Ok(());
        };
        let Some(decoded) = row.get("change").and_then(decode) else {
            tracing::error!(row = ?row.get("seq"), "schedules: an outbox change does not decode; passed over");
            return Ok(());
        };
        match decoded.apply(self.sink.as_ref()).await {
            Err(SchedulerError::InvalidSpec(e)) => {
                let target = decoded.target().key();
                tracing::error!(%target, error = %e, "schedules: the scheduler refused a change as invalid; passed over");
                Ok(())
            }
            applied => applied.map_err(BackendError::new),
        }
    }
}

/// The change in a row's `change` column, which Debezium sends as JSON text.
fn decode(doc: &serde_json::Value) -> Option<ScheduleChange> {
    match doc.as_str() {
        Some(text) => serde_json::from_str(text).ok(),
        None => serde_json::from_value(doc.clone()).ok(),
    }
}
