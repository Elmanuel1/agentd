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

/// Signing keys by id, for rotation: the first signs, every key verifies
/// what it signed. A token names its key's id, so a rotation adds the new key
/// behind the old, moves it first, and drops the old once nothing it signed
/// is live.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyRing {
    keys: Vec<(String, Vec<u8>)>,
}

impl std::fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.keys.iter().map(|(id, _)| id)).finish()
    }
}

/// A key ring that cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct KeyRingError(String);

impl KeyRing {
    /// One key, `k1`.
    pub fn single(key: impl Into<Vec<u8>>) -> Self {
        Self { keys: vec![("k1".into(), key.into())] }
    }

    /// `<id>:<hex>,…`, the signing key first; each id is letters and digits,
    /// each key at least 16 bytes.
    pub fn parse(raw: &str) -> Result<Self, KeyRingError> {
        let keys = raw.split(',').map(str::trim).filter(|k| !k.is_empty()).map(|entry| {
            let bad = || {
                KeyRingError(format!(
                    "`{}…` is not `<id>:<hex of at least 16 bytes>`",
                    entry.chars().take(4).collect::<String>()
                ))
            };
            let (id, key) = entry.split_once(':').ok_or_else(bad)?;
            let key = hex::decode(key).ok().filter(|k| k.len() >= 16).ok_or_else(bad)?;
            match !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
                true => Ok((id.to_owned(), key)),
                false => Err(bad()),
            }
        });
        let keys = keys.collect::<Result<Vec<_>, _>>()?;
        match keys.is_empty() {
            true => Err(KeyRingError("no key".into())),
            false => Ok(Self { keys }),
        }
    }

    /// The ring with each key derived for one purpose.
    #[must_use]
    pub fn derive(&self, purpose: &str) -> Self {
        let keys = self.keys.iter().map(|(id, k)| (id.clone(), hmac_sha256(k, purpose.as_bytes()).to_vec()));
        Self { keys: keys.collect() }
    }

    /// Each key derived for `purpose`, the first key's first.
    pub(crate) fn derived(&self, purpose: &str) -> Vec<[u8; 32]> {
        self.keys.iter().map(|(_, k)| hmac_sha256(k, purpose.as_bytes())).collect()
    }
}

/// Signs claims into `<prefix><key id>.<base64url claims>.<base64url mac>`
/// tokens with a [`KeyRing`]'s first key.
#[derive(Clone)]
pub struct Signer {
    ring: KeyRing,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Signer")
    }
}

impl Signer {
    /// Sign with `ring`.
    pub fn new(ring: KeyRing) -> Self {
        Self { ring }
    }

    /// A signer for one purpose: tokens of different purposes never verify
    /// as each other, even under one key.
    #[must_use]
    pub fn derive(&self, purpose: &str) -> Self {
        Self::new(self.ring.derive(purpose))
    }

    /// A token carrying `claims`.
    pub fn sign(&self, prefix: &str, claims: &impl Serialize) -> String {
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap_or_default());
        let (id, key) = self.ring.keys.first().map(|(id, k)| (id.as_str(), k.as_slice())).unwrap_or_default();
        let signed = format!("{id}.{body}");
        let sig = URL_SAFE_NO_PAD.encode(hmac_sha256(key, signed.as_bytes()));
        format!("{prefix}{signed}.{sig}")
    }

    /// The claims of a token signed with `prefix` by a key of the ring.
    pub fn verify<T: DeserializeOwned>(&self, prefix: &str, token: &str) -> Option<T> {
        let (signed, sig) = token.strip_prefix(prefix)?.rsplit_once('.')?;
        let (id, body) = signed.split_once('.')?;
        let key = self.ring.keys.iter().find(|(k, _)| k == id).map(|(_, key)| key)?;
        let given = URL_SAFE_NO_PAD.decode(sig).ok()?;
        let expected = hmac_sha256(key, signed.as_bytes());
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
        let s = Signer::new(KeyRing::single(b"k".to_vec()));
        let t = s.sign("x-", &vec!["a"]);
        assert_eq!(s.verify::<Vec<String>>("x-", &t), Some(vec!["a".to_owned()]));
        assert_eq!(s.derive("other").verify::<Vec<String>>("x-", &t), None);
        assert_eq!(s.verify::<Vec<String>>("y-", &t), None);
        assert_eq!(s.verify::<Vec<String>>("x-", &t.replace(".", ".A")), None);
    }

    #[test]
    fn a_token_names_its_key_and_verifies_while_that_key_is_in_the_ring() {
        let old = KeyRing::parse("k1:000102030405060708090a0b0c0d0e0f").unwrap();
        let rotated =
            KeyRing::parse("k2:101112131415161718191a1b1c1d1e1f,k1:000102030405060708090a0b0c0d0e0f")
                .unwrap();
        let t = Signer::new(old).sign("x-", &vec!["a"]);
        assert!(t.starts_with("x-k1."));
        let next = Signer::new(rotated);
        assert_eq!(
            next.verify::<Vec<String>>("x-", &t),
            Some(vec!["a".to_owned()]),
            "the old key still verifies"
        );
        assert!(next.sign("x-", &vec!["a"]).starts_with("x-k2."), "the new key signs");
        let retired = Signer::new(KeyRing::parse("k2:101112131415161718191a1b1c1d1e1f").unwrap());
        assert_eq!(retired.verify::<Vec<String>>("x-", &t), None);
        assert!(
            KeyRing::parse("k1:00").is_err()
                && KeyRing::parse("").is_err()
                && KeyRing::parse("k-1:000102030405060708090a0b0c0d0e0f").is_err()
        );
    }
}
