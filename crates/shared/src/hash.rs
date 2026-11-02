//! Content hashing — memory `content_sha256` (lowercase hex, live-verified).

use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 of raw bytes.
pub fn content_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_live_fixture_hash() {
        // Live probe: content "v1 content" (10 bytes) →
        // "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55"
        assert_eq!(
            content_sha256(b"v1 content"),
            "60329bc85480d3c6a566d6101ce5f9fbee836b89e1952cf16326fb392cb59c55"
        );
    }

    #[test]
    fn is_lowercase_hex_64() {
        let h = content_sha256(b"anything");
        assert_eq!(h.len(), 64);
        assert!(h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }
}
