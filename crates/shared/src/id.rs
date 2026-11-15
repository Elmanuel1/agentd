//! Typed resource IDs.
//!
//! Wire grammar (observed live, e.g. `agent_01XdReZgC7eHpg9b9Nvrwdcm`,
//! `sesn_01KZqwuX11cmU6RPvKfze8rR`, `req_011CcfvHuUNNWrHfp5kmZCqv`):
//! `{prefix}_01{22 base62 chars}` — 24 chars after the underscore.
//!
//! `Id<T>` is phantom-typed: an `Id<Agent>` cannot be passed where an
//! `Id<Session>` is expected. Cross-entity references use these typed IDs so
//! entity crates never need to depend on each other for a foreign key.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::str::FromStr;

use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A resource type that owns an ID prefix.
pub trait Resource {
    /// The wire prefix, without the trailing underscore (e.g. `"agent"`).
    const PREFIX: &'static str;
    /// Human label used in the malformed-ID error message (e.g. `"agent"` →
    /// `"Invalid agent ID."`).
    const LABEL: &'static str = Self::PREFIX;
    /// Earlier prefixes whose stored ids still parse; new ids take [`Self::PREFIX`].
    const EARLIER_PREFIXES: &'static [&'static str] = &[];
}

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
/// Canonical generated length of the body after the underscore ("01" + 22).
const BODY_LEN: usize = 24;
/// Liberal accepted body length range (real Anthropic IDs vary slightly).
const BODY_MIN: usize = 20;
const BODY_MAX: usize = 32;

fn gen_body() -> String {
    let mut rng = rand::thread_rng();
    let mut s = String::with_capacity(BODY_LEN);
    s.push('0');
    s.push('1');
    for _ in 0..(BODY_LEN - 2) {
        let i = rng.gen_range(0..BASE62.len());
        // BASE62 is a fixed non-empty table; index is in range by construction.
        s.push(BASE62.get(i).copied().unwrap_or(b'0') as char);
    }
    s
}

/// The body `seed` always gives, in the generated grammar.
fn derived_body(seed: &str) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(seed.as_bytes());
    let chars = digest.iter().take(BODY_LEN - 2).map(|b| {
        let i = usize::from(*b) % BASE62.len();
        BASE62.get(i).copied().unwrap_or(b'0') as char
    });
    "01".chars().chain(chars).collect()
}

fn body_is_valid(body: &str) -> bool {
    (BODY_MIN..=BODY_MAX).contains(&body.len()) && body.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// Error returned when a raw string does not parse as a valid `Id<T>`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid {label} ID.")]
pub struct InvalidId {
    /// Human label of the resource type (e.g. `"agent"`).
    pub label: &'static str,
}

/// Parse a raw id into a typed `Id<T>`, mapping a bad id to the live 400
/// (`"Invalid {label} ID."`). Replaces the per-entity `parse_id` wrappers.
pub fn parse_id<T: Resource>(raw: &str) -> Result<Id<T>, crate::error::ApiError> {
    Id::<T>::parse(raw).map_err(|e| crate::error::ApiError::invalid_request(e.to_string()))
}

/// A typed resource identifier. Serializes as the full prefixed string.
pub struct Id<T: Resource> {
    value: String,
    _marker: PhantomData<fn() -> T>,
}

impl<T: Resource> Id<T> {
    /// Generate a fresh canonical ID (`{prefix}_01{22 base62}`).
    pub fn generate() -> Self {
        Self { value: format!("{}_{}", T::PREFIX, gen_body()), _marker: PhantomData }
    }

    /// The id `seed` always gives, so a create retried with the same seed
    /// finds what the first attempt made.
    pub fn derived(seed: &str) -> Self {
        Self { value: format!("{}_{}", T::PREFIX, derived_body(seed)), _marker: PhantomData }
    }

    /// Parse and validate a raw wire string.
    pub fn parse(raw: &str) -> Result<Self, InvalidId> {
        let rest = std::iter::once(T::PREFIX)
            .chain(T::EARLIER_PREFIXES.iter().copied())
            .find_map(|prefix| raw.strip_prefix(prefix).and_then(|r| r.strip_prefix('_')))
            .ok_or(InvalidId { label: T::LABEL })?;
        if !body_is_valid(rest) {
            return Err(InvalidId { label: T::LABEL });
        }
        Ok(Self { value: raw.to_owned(), _marker: PhantomData })
    }

    /// The full wire string (e.g. `agent_01XdReZgC7eHpg9b9Nvrwdcm`).
    pub fn as_str(&self) -> &str {
        &self.value
    }
}

// Manual impls: derive would bound them on `T: Clone` etc., which is wrong —
// the phantom parameter carries no data.
impl<T: Resource> Clone for Id<T> {
    fn clone(&self) -> Self {
        Self { value: self.value.clone(), _marker: PhantomData }
    }
}
impl<T: Resource> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}
impl<T: Resource> Eq for Id<T> {}
impl<T: Resource> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}
impl<T: Resource> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T: Resource> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.value.cmp(&other.value)
    }
}
impl<T: Resource> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.value)
    }
}
impl<T: Resource> fmt::Display for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.value)
    }
}
impl<T: Resource> FromStr for Id<T> {
    type Err = InvalidId;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}
impl<T: Resource> Serialize for Id<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.value)
    }
}
impl<'de, T: Resource> Deserialize<'de> for Id<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Request IDs (`req_…`) — not a resource, but same grammar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestId(String);

impl RequestId {
    /// Generate a fresh `req_…` id.
    pub fn generate() -> Self {
        Self(format!("req_{}", gen_body()))
    }

    /// The full wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests;
