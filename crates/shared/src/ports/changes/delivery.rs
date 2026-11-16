//! One delivery of a change, whatever carries it (a Valkey stream entry, a
//! Kafka record): the same handling and the same answer to the feed.

use super::{Change, ChangeHandler};

/// Hand the Debezium event `value` to `handler`; `true` once it needs no more
/// delivery: handled, or not a change at all (a tombstone, a heartbeat).
/// `id` names the delivery in the log.
pub async fn deliver(handler: &dyn ChangeHandler, value: Option<&str>, id: &str) -> bool {
    let change = value.and_then(|v| serde_json::from_str(v).ok()).and_then(|v| Change::from_debezium(&v));
    let Some(change) = change else { return true };
    match handler.handle(&change).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(table = %change.table, %id, error = %e, "changes: a handler failed; it runs again");
            false
        }
    }
}
