//! User-supplied `metadata` maps.
//!
//! Live-verified rules (agents): ≤16 entries, key ≤64 chars, value ≤512 chars,
//! with these exact messages:
//! - `invalid metadata: metadata map would have 17 entries, exceeding the maximum of 16`
//! - `invalid metadata: metadata key "…" exceeds maximum length of 64 characters (got 65)`
//! - `invalid metadata: metadata value for key "k" exceeds maximum length of 512 characters (got 513)`
//!
//! **Sessions do not enforce the entry-count limit** (live probe: 9 keys
//! accepted while docs claim max 8) — model that with
//! [`MetadataPolicy::unlimited_entries`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::limits::values;

/// Deterministically-ordered string→string metadata map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Metadata(pub BTreeMap<String, String>);

/// Per-resource metadata rules.
#[derive(Debug, Clone, Copy)]
pub struct MetadataPolicy {
    /// Max entry count, if enforced.
    pub max_entries: Option<usize>,
}

impl MetadataPolicy {
    /// Agent-style policy: 16-entry cap (live-verified).
    pub fn capped() -> Self {
        Self { max_entries: Some(values::METADATA_MAX_ENTRIES) }
    }

    /// Session-style policy: no entry cap (live-verified divergence from docs).
    pub fn unlimited_entries() -> Self {
        Self { max_entries: None }
    }
}

impl Metadata {
    /// Empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate against a policy, producing exact wire error strings.
    pub fn validate(&self, policy: MetadataPolicy) -> Result<(), ApiError> {
        if let Some(max) = policy.max_entries {
            let n = self.0.len();
            if n > max {
                return Err(ApiError::invalid_request(format!(
                    "invalid metadata: metadata map would have {n} entries, exceeding the maximum of {max}"
                )));
            }
        }
        for (k, v) in &self.0 {
            let klen = k.chars().count();
            if klen > values::METADATA_KEY_MAX {
                return Err(ApiError::invalid_request(format!(
                    "invalid metadata: metadata key \"{k}\" exceeds maximum length of {} characters (got {klen})",
                    values::METADATA_KEY_MAX
                )));
            }
            let vlen = v.chars().count();
            if vlen > values::METADATA_VALUE_MAX {
                return Err(ApiError::invalid_request(format!(
                    "invalid metadata: metadata value for key \"{k}\" exceeds maximum length of {} characters (got {vlen})",
                    values::METADATA_VALUE_MAX
                )));
            }
        }
        Ok(())
    }

    /// Entry count.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when the map has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<const N: usize> From<[(&str, &str); N]> for Metadata {
    fn from(pairs: [(&str, &str); N]) -> Self {
        Self(pairs.into_iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(n: usize) -> Metadata {
        Metadata((0..n).map(|i| (format!("k{i}"), "v".to_owned())).collect())
    }

    #[test]
    fn entry_cap_exact_live_string() {
        let e = of(17).validate(MetadataPolicy::capped()).unwrap_err();
        assert_eq!(
            e.message,
            "invalid metadata: metadata map would have 17 entries, exceeding the maximum of 16"
        );
        assert!(of(16).validate(MetadataPolicy::capped()).is_ok());
    }

    #[test]
    fn sessions_skip_entry_cap_like_live_api() {
        // Live probe: session create with 9 metadata keys → 200.
        assert!(of(64).validate(MetadataPolicy::unlimited_entries()).is_ok());
    }

    #[test]
    fn key_length_exact_live_string() {
        let key = "k".repeat(65);
        let m: Metadata = [(key.as_str(), "v")].into();
        let e = m.validate(MetadataPolicy::capped()).unwrap_err();
        assert_eq!(
            e.message,
            format!(
                "invalid metadata: metadata key \"{key}\" exceeds maximum length of 64 characters (got 65)"
            )
        );
    }

    #[test]
    fn value_length_exact_live_string() {
        let val = "v".repeat(513);
        let m: Metadata = [("k", val.as_str())].into();
        let e = m.validate(MetadataPolicy::capped()).unwrap_err();
        assert_eq!(
            e.message,
            "invalid metadata: metadata value for key \"k\" exceeds maximum length of 512 characters (got 513)"
        );
    }

    #[test]
    fn serializes_transparently_as_object() {
        let m: Metadata = [("a", "1")].into();
        assert_eq!(serde_json::to_string(&m).unwrap(), r#"{"a":"1"}"#);
        let empty = Metadata::new();
        assert_eq!(serde_json::to_string(&empty).unwrap(), "{}");
    }
}
