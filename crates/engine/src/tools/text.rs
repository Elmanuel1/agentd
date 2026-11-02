//! Pure text helpers for the file tools, kept apart from sandbox I/O so the
//! rules are tested without a container.

/// Tool output past this many characters is saved to a file in the sandbox,
/// and the model gets a preview and the path (the documented toolset limit).
pub const OUTPUT_LIMIT: usize = 100_000;

/// How much of an oversized output the model sees.
pub const PREVIEW_CHARS: usize = 2_000;

/// `text` with each line prefixed by its 1-indexed number and a tab, from
/// `range` (inclusive; `-1` as the end means the last line).
pub fn numbered(text: &str, range: Option<(i64, i64)>) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let total = i64::try_from(lines.len()).unwrap_or(i64::MAX);
    let (start, end) = match range {
        None if total == 0 => return Ok(String::new()),
        None => (1, total),
        Some((s, -1)) => (s, total),
        Some((s, e)) => (s, e.min(total)),
    };
    if start < 1 || (total > 0 && start > total) || end < start {
        return Err(format!("view_range [{start}, {end}] is outside the file's {total} lines"));
    }
    let mut out = String::new();
    for (n, line) in (start..=end).zip(lines.iter().skip(usize::try_from(start - 1).unwrap_or(0))) {
        out.push_str(&format!("{n}\t{line}\n"));
    }
    Ok(out)
}

/// Replace `old` with `new` in `text`: exactly one match, or every match when
/// `all` is set.
pub fn replace(text: &str, old: &str, new: &str, all: bool) -> Result<(String, usize), String> {
    if old.is_empty() {
        return Err("old_string must not be empty".into());
    }
    let count = text.matches(old).count();
    match (count, all) {
        (0, _) => Err("old_string was not found in the file".into()),
        (1, _) | (_, true) => Ok((text.replace(old, new), count)),
        (n, false) => Err(format!(
            "old_string matches {n} places; give more context to make it unique, or set replace_all"
        )),
    }
}

/// The first `PREVIEW_CHARS` characters of `s`, cut on a char boundary.
pub fn preview(s: &str) -> &str {
    match s.char_indices().nth(PREVIEW_CHARS) {
        Some((i, _)) => s.get(..i).unwrap_or(s),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_matches_the_live_read_tool() {
        assert_eq!(numbered("a\nb\nc\n", None).unwrap(), "1\ta\n2\tb\n3\tc\n");
        assert_eq!(numbered("a\nb\nc", Some((2, -1))).unwrap(), "2\tb\n3\tc\n");
        assert_eq!(numbered("a\nb\nc", Some((2, 9))).unwrap(), "2\tb\n3\tc\n");
        assert!(numbered("a", Some((5, 6))).is_err());
        assert_eq!(numbered("", None).unwrap(), "");
    }

    #[test]
    fn replace_needs_one_match_unless_all() {
        assert_eq!(replace("x y x", "y", "z", false).unwrap(), ("x z x".into(), 1));
        assert!(replace("x y x", "x", "z", false).unwrap_err().contains("matches 2"));
        assert_eq!(replace("x y x", "x", "z", true).unwrap(), ("z y z".into(), 2));
        assert!(replace("x", "q", "z", false).is_err());
    }

    #[test]
    fn preview_cuts_on_a_char_boundary() {
        let s = "é".repeat(PREVIEW_CHARS + 5);
        assert_eq!(preview(&s).chars().count(), PREVIEW_CHARS);
    }
}
