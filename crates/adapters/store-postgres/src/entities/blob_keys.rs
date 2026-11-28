//! Where each kind of row keeps its bytes in the blob store, and the row a key names.

use files::types::FileResource;
use memory::types::{MemoryStoreResource, MemoryVersionResource};
use shared::charset::is_id_charset;
use shared::{Id, Resource};
use skills::{SkillResource, SkillVersionResource};

const SKILLS: &str = "skills/";
const MEMORY: &str = "memory/";

/// The row kind a blob key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    SkillVersion,
    MemoryVersion,
}

/// A file's contents.
pub(crate) fn file(workspace: &str, file: &str) -> String {
    format!("{workspace}/{file}")
}

/// A skill version's bundle, by the version's id: two versions numbered in the same microsecond
/// still get their own objects.
pub(crate) fn skill_version(workspace: &str, skill: &str, version_id: &str) -> String {
    format!("{SKILLS}{workspace}/{skill}/{version_id}")
}

/// A memory version's content.
pub(crate) fn memory_version(workspace: &str, store: &str, version: &str) -> String {
    format!("{MEMORY}{workspace}/{store}/{version}")
}

/// The kind of row `key` belongs to and the parts that name it (a file's third part is empty); none
/// for a key no builder above makes, so the sweep never reads an object agentd did not write as one
/// of its own.
pub(crate) fn parse(key: &str) -> Option<(Kind, [String; 3])> {
    let (kind, rest) = match (key.strip_prefix(SKILLS), key.strip_prefix(MEMORY)) {
        (Some(rest), _) => (Kind::SkillVersion, rest),
        (_, Some(rest)) => (Kind::MemoryVersion, rest),
        _ => (Kind::File, key),
    };
    let parts: Vec<&str> = rest.split('/').collect();
    let owned = |a: &str, b: &str, c: &str| Some((kind, [a.to_owned(), b.to_owned(), c.to_owned()]));
    match (kind, parts.as_slice()) {
        (Kind::File, &[ws, file]) if is_id_charset(ws) && is::<FileResource>(file) => owned(ws, file, ""),
        (Kind::SkillVersion, &[ws, skill, version])
            if is_id_charset(ws) && is::<SkillResource>(skill) && is::<SkillVersionResource>(version) =>
        {
            owned(ws, skill, version)
        }
        (Kind::MemoryVersion, &[ws, store, version])
            if is_id_charset(ws)
                && is::<MemoryStoreResource>(store)
                && is::<MemoryVersionResource>(version) =>
        {
            owned(ws, store, version)
        }
        _ => None,
    }
}

fn is<R: Resource>(id: &str) -> bool {
    Id::<R>::parse(id).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_reads_back_as_the_row_that_made_it_and_no_other_key_does() {
        let parts = |a: &str, b: &str, c: &str| [a.to_owned(), b.to_owned(), c.to_owned()];
        let (ws, file_id) = ("wrkspc_a", "file_01AbCdEfGhIjKlMnOpQrStUv");
        let (skill, skver) = ("skill_01AbCdEfGhIjKlMnOpQrStUv", "skver_01AbCdEfGhIjKlMnOpQrStUv");
        let (store, memver) = ("memstore_01AbCdEfGhIjKlMnOpQrStUv", "memver_01AbCdEfGhIjKlMnOpQrStUv");
        assert_eq!(
            parse(&skill_version(ws, skill, skver)),
            Some((Kind::SkillVersion, parts(ws, skill, skver)))
        );
        assert_eq!(
            parse(&memory_version(ws, store, memver)),
            Some((Kind::MemoryVersion, parts(ws, store, memver)))
        );
        assert_eq!(parse(&file(ws, file_id)), Some((Kind::File, parts(ws, file_id, ""))));
        for foreign in [
            "memory/wrkspc_a/x".to_owned(),
            "a/b/c".to_owned(),
            "backups/db.dump".to_owned(),
            format!("{ws}/file_a"),
            format!("skills/{ws}/skill_a/{skver}"),
            format!("memory/{ws}/{store}/v1"),
            format!("a b/{file_id}"),
        ] {
            assert_eq!(parse(&foreign), None, "{foreign} is not a key agentd writes");
        }
    }
}
