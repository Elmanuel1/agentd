//! history: resource history, an after-effect of the change feed. One
//! [`HistoryRecorder`] per tracked table records each insert and update with
//! the row as committed, its writer stamped on it by the database; the
//! [`DeletedRecorder`] records the deletes the tables announce as
//! `agentd-history` messages, with the deleter as the actor. Each change is
//! recorded at its log position, so a redelivery adds nothing and the order
//! the feed delivers in does not matter.

use std::sync::Arc;
use std::time::Duration;

use shared::ports::changes::{Change, ChangeHandler, Position};
use shared::ports::history::{HistoryError, HistoryLog};
use shared::{BackendError, Timestamp};

mod deleted;
mod recorder;

pub use deleted::DeletedRecorder;
pub use recorder::HistoryRecorder;

/// Told how long after its commit each change was recorded.
pub type Applied = Arc<dyn Fn(Duration) + Send + Sync>;

/// A recorder for each table `log` keeps a history of, and the delete recorder.
pub fn recorders(log: Arc<dyn HistoryLog>, applied: Applied) -> Vec<Arc<dyn ChangeHandler>> {
    let mut handlers: Vec<Arc<dyn ChangeHandler>> = log
        .tables()
        .into_iter()
        .map(|table| Arc::new(HistoryRecorder::new(log.clone(), table, applied.clone())) as _)
        .collect();
    handlers.push(Arc::new(DeletedRecorder::new(log, applied)));
    handlers
}

/// What a recording leaves for the feed: a store failure is handled again; a
/// change that can never be recorded is a gap in history, logged for a
/// person to look at and passed over.
fn settle(
    recorded: Result<(), HistoryError>,
    change: &Change,
    position: Option<&Position>,
    applied: &Applied,
) -> Result<(), BackendError> {
    match recorded {
        Ok(()) => {
            if let Some(p) = position {
                let late = Timestamp::now().as_datetime() - p.committed_at.as_datetime();
                applied(late.to_std().unwrap_or_default());
            }
            Ok(())
        }
        Err(HistoryError::Backend(e)) => Err(e),
        Err(HistoryError::Unrecordable(why)) => {
            let lsn = position.map(|p| p.lsn);
            tracing::error!(table = %change.table, ?lsn, %why, "history: a change cannot be recorded; passed over");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
