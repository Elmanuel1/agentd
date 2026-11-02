//! Masking secrets in a response without holding them: a value is kept only
//! as its length, a rolling hash and a SHA-256 digest, so its plaintext can be
//! zeroed as soon as the request is sent. A window of the response whose
//! rolling hash matches is checked against the digest before it is masked.

use sha2::{Digest, Sha256};

/// The rolling hash's base: odd, so every byte position counts.
const BASE: u64 = 0x0000_0100_0000_01b3;

/// What is kept of one injected secret.
#[derive(Clone)]
pub struct Fingerprint {
    /// The credential's id, for the audit log.
    pub id: String,
    /// What the sandbox sees in its place.
    pub placeholder: String,
    len: usize,
    roll: u64,
    digest: [u8; 32],
}

impl std::fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fingerprint").field("id", &self.id).finish_non_exhaustive()
    }
}

fn roll(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |h, b| h.wrapping_mul(BASE).wrapping_add(u64::from(*b)))
}

impl Fingerprint {
    /// The fingerprint of `value`, which stands for credential `id`.
    pub fn of(id: &str, placeholder: &str, value: &[u8]) -> Self {
        Self {
            id: id.to_owned(),
            placeholder: placeholder.to_owned(),
            len: value.len(),
            roll: roll(value),
            digest: Sha256::digest(value).into(),
        }
    }

    /// `input` with this secret masked, or `None` when it does not occur.
    fn mask(&self, input: &[u8]) -> Option<Vec<u8>> {
        let n = self.len;
        let first = input.get(..n).filter(|_| n > 0)?;
        let pow = (1..n).fold(1u64, |p, _| p.wrapping_mul(BASE));
        let mut out = Vec::with_capacity(input.len());
        let mut found = false;
        let (mut i, mut h) = (0, roll(first));
        while let Some(window) = input.get(i..i + n) {
            if h == self.roll && <[u8; 32]>::from(Sha256::digest(window)) == self.digest {
                out.extend_from_slice(self.placeholder.as_bytes());
                found = true;
                i += n;
                match input.get(i..i + n) {
                    Some(next) => h = roll(next),
                    None => break,
                }
                continue;
            }
            let (Some(&gone), Some(&next)) = (input.get(i), input.get(i + n)) else { break };
            out.push(gone);
            h = h
                .wrapping_sub(u64::from(gone).wrapping_mul(pow))
                .wrapping_mul(BASE)
                .wrapping_add(u64::from(next));
            i += 1;
        }
        out.extend_from_slice(input.get(i..).unwrap_or_default());
        found.then_some(out)
    }
}

/// `input` with every fingerprinted secret masked, and the ids of those found.
pub fn mask(input: &[u8], prints: &[Fingerprint]) -> (Vec<u8>, Vec<String>) {
    let mut out = input.to_vec();
    let mut found = Vec::new();
    for p in prints {
        if let Some(masked) = p.mask(&out) {
            out = masked;
            found.push(p.id.clone());
        }
    }
    (out, found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn print() -> Fingerprint {
        Fingerprint::of("cred_1", "agentd-ph-x", b"ghp_real")
    }

    #[test]
    fn every_occurrence_is_masked_wherever_it_sits() {
        for (input, want) in [
            ("ghp_real", "agentd-ph-x"),
            ("a ghp_real b ghp_real", "a agentd-ph-x b agentd-ph-x"),
            ("ghp_realghp_real", "agentd-ph-xagentd-ph-x"),
            ("xxghp_real", "xxagentd-ph-x"),
        ] {
            let (out, found) = mask(input.as_bytes(), &[print()]);
            assert_eq!(String::from_utf8(out).unwrap(), want, "{input}");
            assert_eq!(found, ["cred_1"]);
        }
    }

    #[test]
    fn text_without_the_secret_is_left_alone() {
        for input in ["", "ghp_rea", "ghp_reaL", "nothing here at all"] {
            let (out, found) = mask(input.as_bytes(), &[print()]);
            assert_eq!(out, input.as_bytes());
            assert!(found.is_empty());
        }
    }

    #[test]
    fn debug_shows_no_digest_or_hash() {
        let shown = format!("{:?}", print());
        assert!(shown.contains("cred_1") && !shown.contains("digest") && !shown.contains("roll"));
    }
}
