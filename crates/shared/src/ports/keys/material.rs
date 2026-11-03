//! API key material: the key a caller holds, the digest agentd stores, and
//! the hint it shows.
//!
//! A key is its kind's prefix and 86 base64url characters (64 random bytes
//! from the OS), then `AA`, the shape of an Anthropic key. With 512 bits of
//! entropy a key cannot be guessed, so a slow password hash (argon2id) would
//! only add cost to every request; the stored digest is HMAC-SHA256 keyed with
//! a pepper kept in Secrets Manager, so the stored digests alone verify nothing.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hmac::{KeyInit, Mac};
use rand::RngCore;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::KeyPepper;
use crate::ports::secrets::SecretString;

const RANDOM_BYTES: usize = 64;
/// 86 base64url characters, then `AA`.
const BODY_LEN: usize = 86 + 2;

type HmacSha256 = hmac::Hmac<Sha256>;

/// The two kinds of key, told apart by prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// A workspace key, `sk-ant-api03-…`.
    Workspace,
    /// An organization admin key, `sk-ant-admin01-…`.
    Admin,
}

impl KeyKind {
    /// The prefix every key of this kind starts with.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Workspace => "sk-ant-api03-",
            Self::Admin => "sk-ant-admin01-",
        }
    }
}

/// A fresh key of `kind`.
#[must_use]
pub fn generate(kind: KeyKind) -> SecretString {
    let mut random = Zeroizing::new([0u8; RANDOM_BYTES]);
    rand::rngs::OsRng.fill_bytes(random.as_mut());
    let mut key = Zeroizing::new(String::with_capacity(kind.prefix().len() + BODY_LEN));
    key.push_str(kind.prefix());
    URL_SAFE_NO_PAD.encode_string(random.as_ref(), &mut key);
    key.push_str("AA");
    SecretString::from(key.as_str())
}

/// Whether `key` has the shape of a `kind` key agentd issues; anything else
/// is refused before a store is asked.
#[must_use]
pub fn well_formed(kind: KeyKind, key: &str) -> bool {
    let prefix = kind.prefix();
    key.len() == prefix.len() + BODY_LEN
        && key.starts_with(prefix)
        && key.bytes().skip(prefix.len()).all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// HMAC takes a key of any length, so this is `None` never in practice.
fn mac(pepper: &KeyPepper, key: &str) -> Option<HmacSha256> {
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(pepper.expose()).ok()?;
    mac.update(key.as_bytes());
    Some(mac)
}

/// The digest stored for `key`, and looked up by.
#[must_use]
pub fn digest(pepper: &KeyPepper, key: &str) -> Option<Vec<u8>> {
    Some(mac(pepper, key)?.finalize().into_bytes().to_vec())
}

/// Whether `stored` is `key`'s digest, compared in constant time.
#[must_use]
pub fn matches(pepper: &KeyPepper, key: &str, stored: &[u8]) -> bool {
    mac(pepper, key).is_some_and(|m| m.verify_slice(stored).is_ok())
}

/// What a listing shows of a key: its first 16 and last 4 characters, as the
/// Anthropic console's `partial_key_hint`.
#[must_use]
pub fn hint(key: &str) -> String {
    let head = key.get(..16).unwrap_or_default();
    let tail = key.get(key.len().saturating_sub(4)..).unwrap_or_default();
    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    use crate::ports::secrets::ExposeSecret;

    use super::*;

    fn pepper(byte: u8) -> KeyPepper {
        KeyPepper::new(&[byte; 32])
    }

    #[test]
    fn a_key_has_the_anthropic_shape_and_is_never_repeated() {
        for kind in [KeyKind::Workspace, KeyKind::Admin] {
            let (a, b) = (generate(kind), generate(kind));
            let a = a.expose_secret();
            assert!(well_formed(kind, a), "{}", a.len());
            assert!(a.starts_with(kind.prefix()) && a.ends_with("AA"));
            assert_ne!(a, b.expose_secret());
        }
    }

    #[test]
    fn a_key_of_one_kind_is_not_well_formed_as_the_other() {
        let admin = generate(KeyKind::Admin);
        let workspace = generate(KeyKind::Workspace);
        assert!(!well_formed(KeyKind::Workspace, admin.expose_secret()));
        assert!(!well_formed(KeyKind::Admin, workspace.expose_secret()));
    }

    #[test]
    fn a_malformed_key_is_refused_before_any_lookup() {
        let key = generate(KeyKind::Workspace);
        let key = key.expose_secret();
        for bad in [
            "",
            "sk-ant-api03-short",
            &key.replace("sk-ant-api03-", "sk-ant-admin-"),
            &format!("{key}x"),
            &key[..key.len() - 1],
            &format!("{}é", &key[..key.len() - 2]),
            &format!("{}/A", &key[..key.len() - 2]),
        ] {
            assert!(!well_formed(KeyKind::Workspace, bad), "{bad}");
        }
    }

    #[test]
    fn the_digest_needs_the_same_pepper_and_the_same_key() {
        let key = generate(KeyKind::Workspace);
        let key = key.expose_secret();
        let stored = digest(&pepper(1), key).unwrap();
        assert_eq!(stored.len(), 32);
        assert!(matches(&pepper(1), key, &stored));
        assert!(!matches(&pepper(2), key, &stored));
        assert!(!matches(&pepper(1), generate(KeyKind::Workspace).expose_secret(), &stored));
        assert!(!matches(&pepper(1), key, &stored[..31]));
    }

    #[test]
    fn the_hint_shows_sixteen_and_four_characters() {
        let key = generate(KeyKind::Workspace);
        let key = key.expose_secret();
        let hint = hint(key);
        assert_eq!(hint.len(), 16 + 3 + 4);
        assert!(hint.starts_with("sk-ant-api03-") && hint.ends_with("AA"));
        assert!(!hint.contains(&key[20..90]));
    }
}
