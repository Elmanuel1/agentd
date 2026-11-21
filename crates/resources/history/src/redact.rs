//! A redacted memory version's content leaves history too: the records stay,
//! never the value (memory decision D2). A memory's head and its versions
//! arrive on separate topics, an unordered stream may record a version's row
//! after its redaction, and a soft-deleted memory's head can be deleted for good
//! after its version is redacted. So every recorded row of either table, its delete
//! included, names the version it may hold, and the scrub that commits with it
//! acts only once history holds the version's redaction.

use serde_json::Value;
use shared::ports::history::HeldVersion;

/// The memory version whose content a recorded `row` of `table` may hold: a
/// version's own row, or the version a memory's head names.
pub(crate) fn held<'a>(table: &str, row: &'a Value) -> Option<HeldVersion<'a>> {
    let field = |k: &str| row.get(k).and_then(Value::as_str);
    let version = match table {
        "memory_versions" => field("version_id"),
        "memories" => field("head_version_id"),
        _ => None,
    }?;
    Some(HeldVersion { workspace: field("workspace_id")?, store: field("store_id")?, version })
}
