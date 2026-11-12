//! The schedule outbox: each change is inserted and deleted again in its
//! deployment write's transaction, so the table stays empty while the WAL
//! keeps the insert. The change feed hands each insert, in commit order, to
//! [`ScheduleOutbox`], which applies it to the scheduler sink.

use std::sync::Arc;

use async_trait::async_trait;
use diesel::sql_types::{Jsonb, Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use shared::ports::changes::{Change, ChangeHandler, ChangeOp};
use shared::ports::{ScheduleChange, SchedulerPort};
use shared::BackendError;

/// Record `change` in the transaction on `c`.
pub(crate) async fn record(
    c: &mut AsyncPgConnection,
    change: &ScheduleChange,
) -> Result<(), diesel::result::Error> {
    let doc =
        serde_json::to_value(change).map_err(|e| diesel::result::Error::SerializationError(e.into()))?;
    diesel::sql_query("WITH added AS (INSERT INTO schedule_outbox (target, change) VALUES ($1, $2) RETURNING seq) DELETE FROM schedule_outbox USING added WHERE schedule_outbox.seq = added.seq")
        .bind::<Text, _>(change.target().key())
        .bind::<Jsonb, _>(doc)
        .execute(c)
        .await
        .map(drop)
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

#[async_trait]
impl ChangeHandler for ScheduleOutbox {
    fn table(&self) -> &'static str {
        "schedule_outbox"
    }

    fn ordered(&self) -> bool {
        true
    }

    /// An insert's change, applied again after a crash, leaves the same
    /// schedule; its delete in the same transaction is not a change to apply.
    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let Some(row) = change.after.as_ref().filter(|_| change.op == ChangeOp::Insert) else {
            return Ok(());
        };
        let Some(doc) = row.get("change") else { return Ok(()) };
        // Debezium sends a jsonb column as its JSON text.
        let doc = match doc.as_str() {
            Some(text) => serde_json::from_str(text).map_err(BackendError::new)?,
            None => doc.clone(),
        };
        let change: ScheduleChange = serde_json::from_value(doc).map_err(BackendError::new)?;
        change.apply(self.sink.as_ref()).await.map_err(BackendError::new)
    }
}
