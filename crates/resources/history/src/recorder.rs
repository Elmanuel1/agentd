//! One tracked table's inserts and updates.

use std::sync::Arc;

use shared::BackendError;
use shared::ports::changes::{Change, ChangeOp, NativeChangeHandler, Source};
use shared::ports::history::{HistoryError, HistoryLog, HistoryOp};

use crate::{Applied, redact, settle};

/// Records each insert and update of one table.
pub struct HistoryRecorder {
    log: Arc<dyn HistoryLog>,
    table: String,
    applied: Applied,
}

impl std::fmt::Debug for HistoryRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryRecorder").field("table", &self.table).finish_non_exhaustive()
    }
}

impl HistoryRecorder {
    /// Recording `table`'s changes into `log`.
    pub fn new(log: Arc<dyn HistoryLog>, table: String, applied: Applied) -> Self {
        Self { log, table, applied }
    }
}

fn missing(what: &str) -> HistoryError {
    HistoryError::Unrecordable(what.to_owned())
}

impl NativeChangeHandler for HistoryRecorder {
    fn source(&self) -> Source {
        Source::Table(self.table.clone())
    }

    /// A delete is recorded from the message its table announces it with,
    /// which names the deleter; the feed's own delete event names the row's
    /// last writer.
    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let op = match change.op {
            ChangeOp::Insert => HistoryOp::Insert,
            ChangeOp::Update => HistoryOp::Update,
            ChangeOp::Delete | ChangeOp::Message => return Ok(()),
        };
        let recorded = async {
            let position = change.position.as_ref().ok_or_else(|| missing("no log position"))?;
            let row = change.after.as_ref().ok_or_else(|| missing("no row after"))?;
            self.log.record(&self.table, op, position, row, redact::held(&self.table, row)).await
        };
        settle(recorded.await, change, &self.applied)
    }
}
