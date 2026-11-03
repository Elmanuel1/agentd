//! agentd's id charset.

/// Whether `raw` is in agentd's id charset: 1 to 64 ASCII letters, digits and
/// underscores. Every id agentd issues or accepts (`{prefix}_{base62}`, and
/// the configured org and workspace ids) is; none can hold a `/`, a `.` or a
/// wildcard.
pub fn is_id_charset(raw: &str) -> bool {
    (1..=64).contains(&raw.len()) && raw.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
