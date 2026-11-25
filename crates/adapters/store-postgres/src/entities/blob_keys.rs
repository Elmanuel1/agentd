//! Where each kind of row keeps its bytes in the blob store.

/// A file's contents.
pub(crate) fn file(workspace: &str, file: &str) -> String {
    format!("{workspace}/{file}")
}

/// A skill version's bundle, by the version's id: two versions numbered in the same microsecond
/// still get their own objects.
pub(crate) fn skill_version(workspace: &str, skill: &str, version_id: &str) -> String {
    format!("skills/{workspace}/{skill}/{version_id}")
}

/// A memory version's content.
pub(crate) fn memory_version(workspace: &str, store: &str, version: &str) -> String {
    format!("memory/{workspace}/{store}/{version}")
}
