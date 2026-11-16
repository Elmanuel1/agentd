//! A redacted memory version's content leaves history too: the records stay,
//! never the value (memory decision D2). A memory's head and its versions
//! arrive on separate topics, an unordered stream may record a version's row
//! after its redaction, and a soft-deleted memory's head can be deleted for good
//! after its version is redacted. So the scrub runs after every recorded row of
//! either table, its delete included, and acts only once history holds the
//! version's redaction.

use serde_json::Value;
use shared::ports::history::{HistoryError, HistoryLog};

/// Scrub the memory version whose content a recorded `row` of `table` may
/// hold: a version's own row, or the version a memory's head names.
pub(crate) async fn scrub(log: &dyn HistoryLog, table: &str, row: &Value) -> Result<(), HistoryError> {
    let field = |k: &str| row.get(k).and_then(Value::as_str);
    let version = match table {
        "memory_versions" => field("version_id"),
        "memories" => field("head_version_id"),
        _ => None,
    };
    match (field("workspace_id"), field("store_id"), version) {
        (Some(workspace), Some(store), Some(version)) => {
            log.scrub_memory_content(workspace, store, version).await
        }
        _ => Ok(()),
    }
}
