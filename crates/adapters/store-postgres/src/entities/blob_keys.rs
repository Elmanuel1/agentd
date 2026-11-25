//! Where each kind of row keeps its bytes in the blob store, and the row a key names.

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
/// for a key no builder above makes.
pub(crate) fn parse(key: &str) -> Option<(Kind, [String; 3])> {
    let (kind, rest) = match (key.strip_prefix(SKILLS), key.strip_prefix(MEMORY)) {
        (Some(rest), _) => (Kind::SkillVersion, rest),
        (_, Some(rest)) => (Kind::MemoryVersion, rest),
        _ => (Kind::File, key),
    };
    let parts: Vec<&str> = rest.split('/').collect();
    match (kind, parts.as_slice()) {
        (Kind::File, [ws, file]) => Some((kind, [(*ws).to_owned(), (*file).to_owned(), String::new()])),
        (Kind::SkillVersion | Kind::MemoryVersion, [ws, owner, version]) => {
            Some((kind, [(*ws).to_owned(), (*owner).to_owned(), (*version).to_owned()]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_key_reads_back_as_the_row_that_made_it() {
        let parts = |a: &str, b: &str, c: &str| [a.to_owned(), b.to_owned(), c.to_owned()];
        let skill = skill_version("wrkspc_a", "skill_a", "skver_a");
        assert_eq!(parse(&skill), Some((Kind::SkillVersion, parts("wrkspc_a", "skill_a", "skver_a"))));
        let memory = memory_version("wrkspc_a", "memstore_a", "memver_a");
        assert_eq!(parse(&memory), Some((Kind::MemoryVersion, parts("wrkspc_a", "memstore_a", "memver_a"))));
        assert_eq!(parse(&file("wrkspc_a", "file_a")), Some((Kind::File, parts("wrkspc_a", "file_a", ""))));
        assert_eq!(parse("memory/wrkspc_a/x"), None);
        assert_eq!(parse("a/b/c"), None);
    }
}
