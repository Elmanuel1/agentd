//! The deletes the tracked tables announce: each `agentd-history` message
//! (V16's `history_deleted()`) is `{table, actor, request_id, row}`, written in
//! the deleting transaction, its row as Postgres's `to_jsonb` gives it.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use shared::ports::changes::{Change, ChangeHandler, ChangeOp, Source};
use shared::ports::history::{HistoryError, HistoryLog, HistoryOp};
use shared::BackendError;

use crate::{settle, Applied};

/// The prefix of the messages a delete emits.
pub const PREFIX: &str = "agentd-history";

/// Records each announced delete, with the deleter as its actor.
pub struct DeletedRecorder {
    log: Arc<dyn HistoryLog>,
    applied: Applied,
}

impl std::fmt::Debug for DeletedRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeletedRecorder").finish_non_exhaustive()
    }
}

impl DeletedRecorder {
    /// Recording into `log`.
    pub fn new(log: Arc<dyn HistoryLog>, applied: Applied) -> Self {
        Self { log, applied }
    }

    async fn record(&self, change: &Change) -> Result<(), HistoryError> {
        let unrecordable = |what: &str| HistoryError::Unrecordable(format!("a delete message with {what}"));
        let position = change.position.as_ref().ok_or_else(|| unrecordable("no log position"))?;
        let message = change.after.as_ref().ok_or_else(|| unrecordable("no content"))?;
        let table = message.get("table").and_then(Value::as_str).ok_or_else(|| unrecordable("no table"))?;
        let mut row =
            message.get("row").and_then(Value::as_object).cloned().ok_or_else(|| unrecordable("no row"))?;
        for writer in ["actor", "request_id"] {
            row.insert(writer.into(), message.get(writer).cloned().unwrap_or(Value::Null));
        }
        self.log.record(table, HistoryOp::Delete, position, &Value::Object(row)).await
    }
}

#[async_trait]
impl ChangeHandler for DeletedRecorder {
    fn source(&self) -> Source {
        Source::Messages
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        if change.op != ChangeOp::Message || change.table != PREFIX {
            return Ok(());
        }
        let recorded = self.record(change).await;
        settle(recorded, change, change.position.as_ref(), &self.applied)
    }
}
