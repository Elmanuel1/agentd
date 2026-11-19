//! The change feed: committed row changes and logical messages read from the
//! database's log (Debezium over the Postgres WAL), handed to the handler of
//! each source. Delivery is at least once, so every handler is idempotent.

use crate::ports::BoxFuture;
use base64::Engine as _;
use serde_json::Value;
use std::future::Future;

use crate::{BackendError, Timestamp};

mod delivery;
pub use delivery::deliver;

/// What a change did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOp {
    /// Inserted (`c`), or read by the initial snapshot (`r`).
    Insert,
    /// Updated (`u`).
    Update,
    /// Deleted (`d`).
    Delete,
    /// A logical message (`m`): its prefix is the change's `table`, its
    /// content the change's `after`.
    Message,
}

/// Where a change sits in the database's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Its log sequence number (`source.lsn`).
    pub lsn: u64,
    /// Its transaction (`source.txId`); none for a message outside one.
    pub tx_id: Option<i64>,
    /// When its transaction committed (`source.ts_ms`).
    pub committed_at: Timestamp,
}

/// One committed change.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// The table, unqualified, or a message's prefix.
    pub table: String,
    /// What happened.
    pub op: ChangeOp,
    /// Where it sits in the log, when the event says.
    pub position: Option<Position>,
    /// The row before, for an update or delete.
    pub before: Option<Value>,
    /// The row after, for an insert or update; a message's content, as JSON
    /// when it parses, else as a string.
    pub after: Option<Value>,
}

impl Change {
    /// A Debezium event's value (`{op, before, after, source}` or
    /// `{op, message, source}`, or the same inside a `payload` envelope);
    /// `None` for anything else, such as a tombstone or a heartbeat.
    pub fn from_debezium(value: &Value) -> Option<Self> {
        let event = value.get("payload").filter(|p| p.is_object()).unwrap_or(value);
        let op = match event.get("op")?.as_str()? {
            "c" | "r" => ChangeOp::Insert,
            "u" => ChangeOp::Update,
            "d" => ChangeOp::Delete,
            "m" => ChangeOp::Message,
            _ => return None,
        };
        let position = event.get("source").and_then(|source| {
            let committed_at = chrono::DateTime::from_timestamp_millis(source.get("ts_ms")?.as_i64()?)?;
            Some(Position {
                lsn: source.get("lsn")?.as_u64()?,
                tx_id: source.get("txId").and_then(Value::as_i64),
                committed_at: Timestamp::from_datetime(committed_at),
            })
        });
        if op == ChangeOp::Message {
            let message = event.get("message")?;
            let prefix = message.get("prefix")?.as_str()?.to_owned();
            let content = message.get("content")?.as_str()?;
            let bytes = base64::engine::general_purpose::STANDARD.decode(content).ok()?;
            let after = match serde_json::from_slice(&bytes) {
                Ok(json) => json,
                Err(_) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
            };
            return Some(Self { table: prefix, op, position, before: None, after: Some(after) });
        }
        let row = |k: &str| event.get(k).filter(|v| !v.is_null()).cloned();
        let table = event.get("source")?.get("table")?.as_str()?.to_owned();
        Some(Self { table, op, position, before: row("before"), after: row("after") })
    }
}

/// What a handler reads from the feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The changes of one table, unqualified.
    Table(String),
    /// The logical messages.
    Messages,
}

impl Source {
    /// Its Kafka topic or Valkey stream among those of Debezium's `prefix`.
    pub fn topic(&self, prefix: &str) -> String {
        match self {
            Self::Table(table) => format!("{prefix}.public.{table}"),
            Self::Messages => format!("{prefix}.message"),
        }
    }
}

/// Reacts to the changes of one source.
pub trait ChangeHandler: Send + Sync {
    /// What it reads.
    fn source(&self) -> Source;
    /// Whether its changes must be handled one at a time, in commit order.
    fn ordered(&self) -> bool;
    /// Handle `change`; an error leaves it to be handled again.
    fn handle<'a>(&'a self, change: &'a Change) -> BoxFuture<'a, Result<(), BackendError>>;
}

/// What an adapter implements to provide [`ChangeHandler`]; the methods are documented there.
pub trait NativeChangeHandler: Send + Sync {
    fn source(&self) -> Source;
    fn ordered(&self) -> bool {
        false
    }
    fn handle(&self, change: &Change) -> impl Future<Output = Result<(), BackendError>> + Send;
}

impl<T: NativeChangeHandler> ChangeHandler for T {
    fn source(&self) -> Source {
        NativeChangeHandler::source(self)
    }
    fn ordered(&self) -> bool {
        NativeChangeHandler::ordered(self)
    }
    fn handle<'a>(&'a self, change: &'a Change) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(NativeChangeHandler::handle(self, change))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_debezium_event_reads_with_or_without_its_schema_envelope() {
        let source = json!({"table": "sessions", "lsn": 24023128, "txId": 771, "ts_ms": 1763290000123_i64});
        let event = json!({"op": "d", "before": {"id": 1}, "after": null, "source": source});
        let at = Timestamp::from_datetime(chrono::DateTime::from_timestamp_millis(1763290000123).unwrap());
        let change = Change {
            table: "sessions".into(),
            op: ChangeOp::Delete,
            position: Some(Position { lsn: 24023128, tx_id: Some(771), committed_at: at }),
            before: Some(json!({"id": 1})),
            after: None,
        };
        assert_eq!(Change::from_debezium(&event), Some(change.clone()));
        assert_eq!(Change::from_debezium(&json!({"schema": {}, "payload": event})), Some(change));
        assert_eq!(Change::from_debezium(&json!("default")), None, "a tombstone");
    }

    #[test]
    fn a_logical_message_decodes_its_base64_content() {
        let content = base64::engine::general_purpose::STANDARD.encode(br#"{"table":"vaults"}"#);
        let event = json!({
            "op": "m",
            "source": {"table": "", "lsn": 7, "txId": 9, "ts_ms": 1},
            "message": {"prefix": "agentd-history", "content": content},
        });
        let at = Timestamp::from_datetime(chrono::DateTime::from_timestamp_millis(1).unwrap());
        let expected = Change {
            table: "agentd-history".into(),
            op: ChangeOp::Message,
            position: Some(Position { lsn: 7, tx_id: Some(9), committed_at: at }),
            before: None,
            after: Some(json!({"table": "vaults"})),
        };
        assert_eq!(Change::from_debezium(&event), Some(expected));
        let text = base64::engine::general_purpose::STANDARD.encode(b"2026-11-16 10:00:00+00");
        let other = json!({"op": "m", "message": {"prefix": "agentd-heartbeat", "content": text}});
        let expected = Change {
            table: "agentd-heartbeat".into(),
            op: ChangeOp::Message,
            position: None,
            before: None,
            after: Some(json!("2026-11-16 10:00:00+00")),
        };
        assert_eq!(Change::from_debezium(&other), Some(expected));
    }

    #[test]
    fn a_heartbeat_is_not_a_change() {
        assert_eq!(Change::from_debezium(&json!({"ts_ms": 1763290000123_i64})), None);
    }

    #[test]
    fn a_source_names_its_topic() {
        assert_eq!(Source::Table("vaults".into()).topic("agentd"), "agentd.public.vaults");
        assert_eq!(Source::Messages.topic("agentd"), "agentd.message");
    }
}
