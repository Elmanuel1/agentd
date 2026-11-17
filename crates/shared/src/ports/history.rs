//! Resource history: every committed change of the tracked tables, recorded
//! from the change feed with where it sits in the log. Each adapter owns its
//! storage's shape; Postgres keeps one history table per tracked table.

use async_trait::async_trait;
use serde_json::Value;

use super::changes::Position;
use crate::BackendError;

/// What a recorded change did to its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryOp {
    /// Inserted.
    Insert,
    /// Updated.
    Update,
    /// Deleted.
    Delete,
}

impl HistoryOp {
    /// The stored name: `insert`, `update` or `delete`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// Why a change was not recorded.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    /// The store failed; recording the change again may succeed.
    #[error("history store: {0}")]
    Backend(#[from] BackendError),
    /// The change can never be recorded as it is: a table with no history,
    /// or a row that does not decode.
    #[error("history change: {0}")]
    Unrecordable(String),
}

/// Where history is kept.
#[async_trait]
pub trait HistoryLog: Send + Sync {
    /// The tables it keeps a history of.
    fn tables(&self) -> Vec<String>;
    /// Record `row` as `table`'s change at `position`, once however often it
    /// is recorded. An insert's or update's row is as the feed encodes it
    /// (Debezium's JSON); a delete's is Postgres's own `to_jsonb` of the row.
    async fn record(
        &self,
        table: &str,
        op: HistoryOp,
        position: &Position,
        row: &Value,
    ) -> Result<(), HistoryError>;
    /// When history records memory version `version` of `store` as redacted,
    /// remove its content from the version's history and from the history of
    /// every head of its memory that held it; otherwise change nothing.
    async fn scrub_memory_content(
        &self,
        workspace: &str,
        store: &str,
        version: &str,
    ) -> Result<(), HistoryError>;
}
