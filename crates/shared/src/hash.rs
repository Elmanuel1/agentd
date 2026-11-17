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
        Ok(Self { keys: entries(raw, "at least 16 bytes", |k| (k.len() >= 16).then_some(k))? })
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

/// The entries of a key ring `<id>:<hex>,…`, each key kept as `key` makes it
/// (`None` refuses it). An error names an entry by its position, never its
/// text: an entry typed `<hex>:<id>` would print the key as its id.
fn entries<K>(
    raw: &str,
    size: &str,
    key: impl Fn(Vec<u8>) -> Option<K>,
) -> Result<Vec<(String, K)>, KeyRingError> {
    let entries = raw.split(',').map(str::trim).filter(|k| !k.is_empty()).enumerate();
    let keys = entries.map(|(i, entry)| {
        let bad = || KeyRingError(format!("entry {} is not `<id>:<hex of {size}>`", i + 1));
        let (id, hex) = entry.split_once(':').ok_or_else(bad)?;
        let key = hex::decode(hex).ok().and_then(&key).ok_or_else(bad)?;
        match !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
            true => Ok((id.to_owned(), key)),
            false => Err(bad()),
        }
    });
    let keys = keys.collect::<Result<Vec<_>, _>>()?;
    match keys.is_empty() {
        true => Err(KeyRingError("no key".into())),
        false => Ok(keys),
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

mod secret_keys;
pub use secret_keys::SecretKeys;

#[cfg(test)]
mod tests;
