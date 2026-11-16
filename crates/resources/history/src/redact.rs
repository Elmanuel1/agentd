//! A redacted memory version's content leaves history too: the records stay,
//! never the value (memory decision D2). A memory's head and its versions
//! arrive on separate topics in either order, so the scrub runs after each
//! version update and after each head change, and acts only once history holds
//! the version's redaction.

use serde_json::Value;
use shared::ports::changes::{Change, ChangeOp};

/// The memory version `(workspace, store, version)` whose content `change`
/// may have left in history: a version's update, or the version a memory's
/// head names.
pub(crate) fn scrubbed(change: &Change) -> Option<(String, String, String)> {
    let row = change.after.as_ref()?;
    let field = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_owned);
    let version = match (change.table.as_str(), change.op) {
        ("memory_versions", ChangeOp::Update) => field("version_id")?,
        ("memories", ChangeOp::Insert | ChangeOp::Update) => field("head_version_id")?,
        _ => return None,
    };
    Some((field("workspace_id")?, field("store_id")?, version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn change(table: &str, op: ChangeOp, after: Value) -> Change {
        Change { table: table.into(), op, position: None, before: None, after: Some(after) }
    }

    #[test]
    fn a_version_update_and_a_head_change_name_the_version_to_scrub() {
        let ids = ("wrkspc_1".to_owned(), "memstore_1".to_owned(), "memver_1".to_owned());
        let row = json!({"workspace_id": "wrkspc_1", "store_id": "memstore_1", "version_id": "memver_1"});
        assert_eq!(scrubbed(&change("memory_versions", ChangeOp::Update, row.clone())), Some(ids.clone()));
        assert_eq!(scrubbed(&change("memory_versions", ChangeOp::Insert, row)), None, "a new version");
        let head =
            json!({"workspace_id": "wrkspc_1", "store_id": "memstore_1", "head_version_id": "memver_1"});
        assert_eq!(scrubbed(&change("memories", ChangeOp::Insert, head.clone())), Some(ids));
        assert_eq!(scrubbed(&change("vaults", ChangeOp::Update, head)), None);
    }
}
