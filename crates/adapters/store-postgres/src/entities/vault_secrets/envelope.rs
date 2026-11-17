//! Envelope encryption of one vault secret: the value sealed with AES-256-GCM
//! under a fresh data key, the data key sealed under a single-use wrap key
//! from the master key ring ([`SecretKeys::wrap_key`]). It runs no statement.

use aws_lc_rs::aead::{Aad, Nonce, RandomizedNonceKey, AES_256_GCM, NONCE_LEN};
use diesel::{Queryable, QueryableByName, Selectable};
use shared::hash::SecretKeys;
use shared::ports::secrets::{into_secret, ExposeSecret, SecretError, SecretKey, SecretString};
use zeroize::{Zeroize, Zeroizing};

const SALT_LEN: usize = 32;
const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;

/// One secret as `vault_secrets` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Queryable, QueryableByName, Selectable)]
#[diesel(table_name = crate::schema::vault_secrets, check_for_backend(diesel::pg::Pg))]
pub(crate) struct Sealed {
    pub(crate) environment_variable: bool,
    /// The master key the data key is wrapped under.
    pub(crate) key_id: String,
    /// Salt (32), nonce (12), wrapped data key (32), tag (16).
    pub(crate) sealed_key: Vec<u8>,
    /// Nonce (12), encrypted value, tag (16).
    pub(crate) sealed_value: Vec<u8>,
}

/// `value` sealed for `key` under a new data key, wrapped under the ring's first key.
pub(crate) fn seal(keys: &SecretKeys, key: &SecretKey, value: &SecretString) -> Result<Sealed, SecretError> {
    let failed = || SecretError::Backend(format!("vault secret {key} could not be sealed"));
    let environment_variable = key.is_environment_variable();
    let aad = aad(key, environment_variable);
    let (mut data_key, mut salt) = (Zeroizing::new([0u8; KEY_LEN]), [0u8; SALT_LEN]);
    aws_lc_rs::rand::fill(data_key.as_mut_slice()).map_err(|_| failed())?;
    aws_lc_rs::rand::fill(&mut salt).map_err(|_| failed())?;
    let sealed_value =
        seal_with(data_key.as_slice(), &aad, value.expose_secret().as_bytes()).ok_or_else(failed)?;
    let (key_id, wrap) = keys.wrap_key(None, &salt).ok_or_else(failed)?;
    let wrapped = seal_with(wrap.as_slice(), &aad, data_key.as_slice()).ok_or_else(failed)?;
    let sealed_key = [salt.as_slice(), &wrapped].concat();
    Ok(Sealed { environment_variable, key_id: key_id.to_owned(), sealed_key, sealed_value })
}

/// The secret `row` holds for `key`, opened with the ring key the row names.
/// The kind comes from the row: a reader asks with a plain key.
pub(crate) fn open(keys: &SecretKeys, key: &SecretKey, row: &Sealed) -> Result<SecretString, SecretError> {
    let fails = || SecretError::Backend(format!("vault secret {key} does not open with key {}", row.key_id));
    let aad = aad(key, row.environment_variable);
    let (salt, wrapped) = row.sealed_key.split_at_checked(SALT_LEN).ok_or_else(fails)?;
    let (_, wrap) = keys.wrap_key(Some(&row.key_id), salt).ok_or_else(fails)?;
    let data_key = open_with(wrap.as_slice(), &aad, wrapped).ok_or_else(fails)?;
    let mut plain = open_with(&data_key, &aad, &row.sealed_value).ok_or_else(fails)?;
    match String::from_utf8(std::mem::take(&mut *plain)) {
        Ok(text) => Ok(into_secret(text)),
        Err(e) => {
            e.into_bytes().zeroize();
            Err(fails())
        }
    }
}

/// `agentd/vault-secret/v1/<workspace>/<vault>/<credential>/<env|mcp>`. The
/// ids are in the id charset, which has no `/`, so the layout is unambiguous:
/// a row moved to another credential, or with its kind flipped, does not open.
fn aad(key: &SecretKey, environment_variable: bool) -> Vec<u8> {
    let kind = match environment_variable {
        true => "env",
        false => "mcp",
    };
    format!("agentd/vault-secret/v1/{key}/{kind}").into_bytes()
}

/// Nonce, ciphertext and tag of `plain` under `key`. The buffer is sized for
/// the tag, so sealing never reallocates and leaves a plaintext copy behind.
fn seal_with(key: &[u8], aad: &[u8], plain: &[u8]) -> Option<Vec<u8>> {
    let key = RandomizedNonceKey::new(&AES_256_GCM, key).ok()?;
    let mut buf = Zeroizing::new(Vec::with_capacity(plain.len() + TAG_LEN));
    buf.extend_from_slice(plain);
    let nonce = key.seal_in_place_append_tag(Aad::from(aad), &mut *buf).ok()?;
    Some([nonce.as_ref().as_slice(), &buf].concat())
}

/// The plaintext of `sealed` (nonce, ciphertext, tag) under `key`.
fn open_with(key: &[u8], aad: &[u8], sealed: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    let key = RandomizedNonceKey::new(&AES_256_GCM, key).ok()?;
    let (nonce, body) = sealed.split_at_checked(NONCE_LEN)?;
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut buf = Zeroizing::new(body.to_vec());
    let len = key.open_in_place(nonce, Aad::from(aad), buf.as_mut_slice()).ok()?.len();
    buf.truncate(len);
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "k1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const K2: &str = "k2:202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";
    const VALUE: &str = "xoxb-known-secret-value";

    fn ring(raw: &str) -> SecretKeys {
        SecretKeys::parse(raw).unwrap()
    }

    fn key(credential: &str) -> SecretKey {
        SecretKey::new("wrkspc_01a", "vlt_01b", credential).unwrap()
    }

    #[test]
    fn a_seal_opens_and_two_seals_of_one_value_share_nothing() {
        let (keys, key) = (ring(K1), key("cred_01c"));
        let a = seal(&keys, &key, &SecretString::from(VALUE)).unwrap();
        let b = seal(&keys, &key, &SecretString::from(VALUE)).unwrap();
        assert_eq!(open(&keys, &key, &a).unwrap().expose_secret(), VALUE);
        assert_eq!((a.key_id.as_str(), a.sealed_key.len(), a.sealed_value.len()), ("k1", 92, 12 + 23 + 16));
        assert_ne!(a.sealed_key[..32], b.sealed_key[..32], "salt");
        assert_ne!(a.sealed_key[32..44], b.sealed_key[32..44], "wrap nonce");
        assert_ne!(a.sealed_value[..12], b.sealed_value[..12], "value nonce");
        assert_ne!(a.sealed_value[12..], b.sealed_value[12..], "ciphertext");
    }

    #[test]
    fn any_changed_byte_or_other_context_fails_to_open() {
        let (keys, key) = (ring(K1), key("cred_01c").environment_variable());
        let row = seal(&keys, &key, &SecretString::from(VALUE)).unwrap();
        for i in 0..row.sealed_key.len() {
            let mut bad = row.clone();
            bad.sealed_key[i] ^= 1;
            assert!(open(&keys, &key, &bad).is_err(), "sealed_key byte {i}");
        }
        for i in 0..row.sealed_value.len() {
            let mut bad = row.clone();
            bad.sealed_value[i] ^= 1;
            assert!(open(&keys, &key, &bad).is_err(), "sealed_value byte {i}");
        }
        let elsewhere = [
            SecretKey::new("wrkspc_01x", "vlt_01b", "cred_01c").unwrap(),
            SecretKey::new("wrkspc_01a", "vlt_01x", "cred_01c").unwrap(),
            SecretKey::new("wrkspc_01a", "vlt_01b", "cred_01x").unwrap(),
        ];
        for other in elsewhere {
            assert!(open(&keys, &other, &row).is_err(), "{other}");
        }
        let flipped = Sealed { environment_variable: false, ..row.clone() };
        assert!(open(&keys, &key, &flipped).is_err(), "the kind is in the associated data");
        assert_eq!(
            open(&keys, &self::key("cred_01c"), &row).unwrap().expose_secret(),
            VALUE,
            "a plain key reads"
        );
    }

    #[test]
    fn a_row_opens_while_its_key_is_in_the_ring_and_errors_name_no_material() {
        let key = key("cred_01c");
        let row = seal(&ring(K1), &key, &SecretString::from(VALUE)).unwrap();
        assert_eq!(open(&ring(&format!("{K2},{K1}")), &key, &row).unwrap().expose_secret(), VALUE);
        let err = open(&ring(K2), &key, &row).unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(matches!(err, SecretError::Backend(_)));
        assert_eq!(
            err.to_string(),
            "secret backend: vault secret wrkspc_01a/vlt_01b/cred_01c does not open with key k1"
        );
        assert!(!shown.contains(VALUE) && !shown.contains("000102") && !shown.contains("202122"), "{shown}");
    }
}
