//! Hashing: memory `content_sha256` (lowercase hex, live-verified), and the
//! signed tokens agentd issues (work tokens, egress proxy passwords).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Lowercase-hex SHA-256 of raw bytes.
pub fn content_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let hashed;
    let key = if key.len() > 64 {
        hashed = Sha256::digest(key);
        hashed.as_slice()
    } else {
        key
    };
    let mut block = [0u8; 64];
    for (b, k) in block.iter_mut().zip(key) {
        *b = *k;
    }
    let pad = |b: u8| block.iter().map(|k| k ^ b).collect::<Vec<u8>>();
    let inner = Sha256::new().chain_update(pad(0x36)).chain_update(message).finalize();
    Sha256::new().chain_update(pad(0x5c)).chain_update(inner).finalize().into()
}

/// Signs claims into `<prefix><base64url claims>.<base64url mac>` tokens.
#[derive(Clone)]
pub struct Signer {
    key: Vec<u8>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Signer")
    }
}

impl Signer {
    /// Sign with `key`.
    pub fn new(key: impl Into<Vec<u8>>) -> Self {
        Self { key: key.into() }
    }

    /// A signer for one purpose: tokens of different purposes never verify
    /// as each other, even under one key.
    #[must_use]
    pub fn derive(&self, purpose: &str) -> Self {
        Self::new(hmac_sha256(&self.key, purpose.as_bytes()).to_vec())
    }

    /// The MAC of `message`, hex.
    pub fn mac_hex(&self, message: &str) -> String {
        hex::encode(hmac_sha256(&self.key, message.as_bytes()))
    }

    /// A token carrying `claims`.
    pub fn sign(&self, prefix: &str, claims: &impl Serialize) -> String {
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap_or_default());
        let sig = URL_SAFE_NO_PAD.encode(hmac_sha256(&self.key, body.as_bytes()));
        format!("{prefix}{body}.{sig}")
    }

    /// The claims of a token this signer made with `prefix`.
    pub fn verify<T: DeserializeOwned>(&self, prefix: &str, token: &str) -> Option<T> {
        let (body, sig) = token.strip_prefix(prefix)?.split_once('.')?;
        let given = URL_SAFE_NO_PAD.decode(sig).ok()?;
        let expected = hmac_sha256(&self.key, body.as_bytes());
        // Constant time: a timing difference must not reveal a prefix.
        if given.len() != expected.len()
            || given.iter().zip(expected).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0
        {
            return None;
        }
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()
    }
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

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex::encode(mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn a_token_verifies_only_unchanged_under_its_signer_and_purpose() {
        let s = Signer::new(b"k".to_vec());
        let t = s.sign("x-", &vec!["a"]);
        assert_eq!(s.verify::<Vec<String>>("x-", &t), Some(vec!["a".to_owned()]));
        assert_eq!(s.derive("other").verify::<Vec<String>>("x-", &t), None);
        assert_eq!(s.verify::<Vec<String>>("y-", &t), None);
        assert_eq!(s.verify::<Vec<String>>("x-", &t.replace(".", ".A")), None);
    }
}
