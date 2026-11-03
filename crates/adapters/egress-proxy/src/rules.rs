//! What the proxy knows of a session's credentials before a request uses
//! them (names, placeholders and where their values live, never a value),
//! and which hosts a rule matches.

use shared::ports::secrets::SecretKey;

/// A credential bound to a session: the sandbox sees `name` set to
/// `placeholder`; the value stays in the store at `key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The vault credential's id, for the audit log.
    pub id: String,
    /// The environment variable name the sandbox sees.
    pub name: String,
    /// The opaque per-session stand-in the sandbox sees.
    pub placeholder: String,
    /// Where the value is stored.
    pub key: SecretKey,
}

impl Binding {
    /// Whether `bytes` carry this credential's placeholder.
    pub fn found_in(&self, bytes: &[u8]) -> bool {
        let p = self.placeholder.as_bytes();
        !p.is_empty() && bytes.windows(p.len()).any(|w| w == p)
    }
}

/// Whether `host` matches `rule`: a bare host matches only itself;
/// `*.example.com` matches every subdomain of `example.com`, not the apex.
pub fn host_matches(rule: &str, host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let rule = rule.trim_end_matches('.').to_ascii_lowercase();
    match rule.strip_prefix("*.") {
        Some(apex) => host.len() > apex.len() + 1 && host.ends_with(&format!(".{apex}")),
        None => rule == host,
    }
}

/// Whether any of `rules` matches `host`.
pub fn any_matches(rules: &[String], host: &str) -> bool {
    rules.iter().any(|r| host_matches(r, host))
}

/// `haystack` with every `from` replaced by `to`, or `None` when `from` does
/// not occur.
pub fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Option<Vec<u8>> {
    if from.is_empty() || !haystack.windows(from.len()).any(|w| w == from) {
        return None;
    }
    let mut out = Vec::with_capacity(haystack.len() + to.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack.get(i..i + from.len()) == Some(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.extend_from_slice(haystack.get(i..=i).unwrap_or_default());
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
