//! The change feed: committed row changes read from the database's log
//! (Debezium over the Postgres WAL), handed to the handler of each table.
//! Delivery is at least once, so every handler is idempotent.

use async_trait::async_trait;
use serde_json::Value;

use crate::BackendError;

mod delivery;
pub use delivery::deliver;

/// What a change did to its row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOp {
    /// Inserted (`c`), or read by the initial snapshot (`r`).
    Insert,
    /// Updated (`u`).
    Update,
    /// Deleted (`d`).
    Delete,
}

/// One committed row change.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The table, unqualified.
    pub table: String,
    /// What happened.
    pub op: ChangeOp,
    /// The row before, for an update or delete.
    pub before: Option<Value>,
    /// The row after, for an insert or update.
    pub after: Option<Value>,
}

impl Change {
    /// A Debezium change event's value (`{op, before, after, source}`, or
    /// the same inside a `payload` envelope); `None` for anything else, such
    /// as a tombstone or a heartbeat.
    pub fn from_debezium(value: &Value) -> Option<Self> {
        let event = value.get("payload").filter(|p| p.is_object()).unwrap_or(value);
        let op = match event.get("op")?.as_str()? {
            "c" | "r" => ChangeOp::Insert,
            "u" => ChangeOp::Update,
            "d" => ChangeOp::Delete,
            _ => return None,
        };
        let row = |k: &str| event.get(k).filter(|v| !v.is_null()).cloned();
        let table = event.get("source")?.get("table")?.as_str()?.to_owned();
        Some(Self { table, op, before: row("before"), after: row("after") })
    }
}

/// Reacts to the changes of one table.
#[async_trait]
pub trait ChangeHandler: Send + Sync {
    /// The table it reads.
    fn table(&self) -> &'static str;
    /// Whether its changes must be handled one at a time, in commit order.
    fn ordered(&self) -> bool {
        false
    }
    /// Handle `change`; an error leaves it to be handled again.
    async fn handle(&self, change: &Change) -> Result<(), BackendError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_debezium_event_reads_with_or_without_its_schema_envelope() {
        let event = json!({"op": "d", "before": {"id": 1}, "after": null, "source": {"table": "sessions"}});
        let change = Change::from_debezium(&event).unwrap();
        assert_eq!(
            (change.op, change.table.as_str(), change.after.clone()),
            (ChangeOp::Delete, "sessions", None)
        );
        assert_eq!(Change::from_debezium(&json!({"schema": {}, "payload": event})), Some(change));
        assert_eq!(Change::from_debezium(&json!("default")), None, "a tombstone");
    }
}
