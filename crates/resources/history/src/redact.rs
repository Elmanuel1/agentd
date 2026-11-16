//! A redacted memory version's content leaves history too: the records stay,
//! never the value (memory decision D2). A memory's head and its versions
//! arrive on separate topics, an unordered stream may record a version's row
//! after its redaction, and a soft-deleted memory's head can be deleted for good
//! after its version is redacted. So the scrub runs after every recorded row of
//! either table, its delete included, and acts only once history holds the
//! version's redaction.

use serde_json::Value;

/// The memory version `(workspace, store, version)` whose content a recorded
/// `row` of `table` may hold: a version's own row, or the version a memory's
/// head names.
pub(crate) fn scrubbed(table: &str, row: &Value) -> Option<(String, String, String)> {
    let field = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_owned);
    let version = match table {
        "memory_versions" => field("version_id")?,
        "memories" => field("head_version_id")?,
        _ => return None,
    };
    Some((field("workspace_id")?, field("store_id")?, version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_version_row_and_a_head_row_name_the_version_to_scrub() {
        let ids = Some(("wrkspc_1".to_owned(), "memstore_1".to_owned(), "memver_1".to_owned()));
        let row = json!({"workspace_id": "wrkspc_1", "store_id": "memstore_1", "version_id": "memver_1"});
        assert_eq!(scrubbed("memory_versions", &row), ids);
        let head =
            json!({"workspace_id": "wrkspc_1", "store_id": "memstore_1", "head_version_id": "memver_1"});
        assert_eq!(scrubbed("memories", &head), ids);
        assert_eq!(scrubbed("vaults", &head), None);
    }
}
