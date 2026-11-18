//! The master keys vault secrets are sealed under (`AGENTD_SECRET_KEYS`).

use aws_lc_rs::{aead, hkdf};
use zeroize::{Zeroize, Zeroizing};

use super::{entries, KeyRingError};

/// The HKDF `info` of a wrap key: a key derived for anything else never wraps.
const WRAP_INFO: &[u8] = b"agentd/vault-secret/wrap/v1";

/// Master keys by id, for rotation: the first wraps new data keys, every one
/// unwraps what it wrapped. No key's bytes leave the ring: it gives out only
/// keys derived for one wrap.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKeys {
    keys: Vec<(String, Zeroizing<[u8; 32]>)>,
}

impl std::fmt::Debug for SecretKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.keys.iter().map(|(id, _)| id)).finish()
    }
}

impl SecretKeys {
    /// `<id>:<hex>,…`, the wrapping key first; each id is letters and digits,
    /// each key exactly 32 bytes.
    pub fn parse(raw: &str) -> Result<Self, KeyRingError> {
        let keys = entries(raw, "exactly 32 bytes", |mut key| {
            let fixed = <[u8; 32]>::try_from(key.as_slice()).ok().map(Zeroizing::new);
            key.zeroize();
            fixed
        })?;
        Ok(Self { keys })
    }

    /// The id of the key new data keys are wrapped under.
    pub fn first_id(&self) -> &str {
        self.keys.first().map(|(id, _)| id.as_str()).unwrap_or_default()
    }

    /// The key that wraps one data key, with the id of the master key it
    /// comes from: the key `id` names, or the first with `None`; `None` when
    /// the ring holds no such key.
    pub fn wrap_key(&self, id: Option<&str>, salt: &[u8]) -> Option<(&str, Zeroizing<[u8; 32]>)> {
        let (id, key) = match id {
            None => self.keys.first(),
            Some(id) => self.keys.iter().find(|(k, _)| k == id),
        }?;
        // HKDF-SHA256 (RFC 5869) with a fresh salt per wrap: each wrap key
        // encrypts once, so the AES-GCM limit of 2^32 random nonces per key
        // never applies to a master key.
        let mut wrap = Zeroizing::new([0u8; 32]);
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(key.as_slice());
        prk.expand(&[WRAP_INFO], &aead::AES_256_GCM).ok()?.fill(wrap.as_mut_slice()).ok()?;
        Some((id.as_str(), wrap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const K2: &str = "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";

    #[test]
    fn the_first_key_wraps_and_every_key_derives_its_own() {
        let ring = SecretKeys::parse(&format!("k2:{K2},k1:{K1}")).unwrap();
        assert_eq!(ring.first_id(), "k2");
        let (id, first) = ring.wrap_key(None, b"salt").unwrap();
        assert_eq!(id, "k2");
        assert_eq!(*ring.wrap_key(Some("k2"), b"salt").unwrap().1, *first);
        assert_ne!(*ring.wrap_key(Some("k1"), b"salt").unwrap().1, *first);
        assert_ne!(*ring.wrap_key(None, b"other").unwrap().1, *first, "a new salt is a new key");
        assert!(ring.wrap_key(Some("k0"), b"salt").is_none());
        assert_eq!(format!("{ring:?}"), r#"["k2", "k1"]"#);
    }

    #[test]
    fn a_bad_entry_is_named_by_its_position_never_its_text() {
        let short = &K1[..32];
        for (raw, entry) in [
            (format!("k1:{short}"), 1),
            (format!("k1:{K1},{K2}:k2"), 2),
            (format!("k1:{K1},k2{K2}"), 2),
            (format!("k-1:{K1}"), 1),
        ] {
            let err = SecretKeys::parse(&raw).unwrap_err().to_string();
            assert_eq!(err, format!("entry {entry} is not `<id>:<hex of exactly 32 bytes>`"));
        }
        assert_eq!(SecretKeys::parse(" , ").unwrap_err().to_string(), "no key");
        let repeated = SecretKeys::parse(&format!("k1:{K2},k1:{K1}")).unwrap_err().to_string();
        assert_eq!(repeated, "entry 2 repeats an earlier id", "a rotation that reuses an id is refused");
    }
}
