//! Update bodies tell three cases apart: a field left out keeps its value, a
//! field sent as `null` takes its default, and a field sent with a value
//! replaces it.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

use crate::Metadata;

/// One field of an update body. Use with `#[serde(default)]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Patch<T> {
    /// Left out: keep the stored value.
    #[default]
    Keep,
    /// Sent as `null`: take the default.
    Clear,
    /// Sent with a value: replace.
    Set(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<T>::deserialize(d)?.map_or(Self::Clear, Self::Set))
    }
}

impl<T> Patch<T> {
    /// The value after the update: `current` kept, `default()` on clear.
    pub fn apply(self, current: T, default: impl FnOnce() -> T) -> T {
        match self {
            Self::Keep => current,
            Self::Clear => default(),
            Self::Set(v) => v,
        }
    }
}

impl<T> Patch<T> {
    /// The value after the update of a nullable field: `null` clears it.
    pub fn apply_to_option(self, current: Option<T>) -> Option<T> {
        match self {
            Self::Keep => current,
            Self::Clear => None,
            Self::Set(v) => Some(v),
        }
    }
}

/// A metadata update: each key sent with a string is upserted, each sent as
/// `null` (or `""`) is removed, and keys left out are kept.
pub type MetadataPatch = BTreeMap<String, Option<String>>;

impl Metadata {
    /// Merge `patch` into this map; `Patch::Clear` empties it.
    #[must_use]
    pub fn merged(mut self, patch: Patch<MetadataPatch>) -> Self {
        match patch {
            Patch::Keep => self,
            Patch::Clear => Self::default(),
            Patch::Set(entries) => {
                for (key, value) in entries {
                    match value.filter(|v| !v.is_empty()) {
                        Some(v) => {
                            self.0.insert(key, v);
                        }
                        None => {
                            self.0.remove(&key);
                        }
                    }
                }
                self
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Body {
        #[serde(default)]
        description: Patch<String>,
    }

    #[test]
    fn absent_null_and_value_are_three_cases() {
        let parse = |s: &str| serde_json::from_str::<Body>(s).unwrap().description;
        assert_eq!(parse("{}"), Patch::Keep);
        assert_eq!(parse(r#"{"description":null}"#), Patch::Clear);
        assert_eq!(parse(r#"{"description":"d"}"#), Patch::Set("d".into()));
    }

    #[test]
    fn metadata_merges_upserts_and_removes() {
        let stored = Metadata::from([("a", "1"), ("b", "2")]);
        let patch: MetadataPatch = serde_json::from_str(r#"{"a":"9","b":null,"c":"3","d":""}"#).unwrap();
        assert_eq!(stored.clone().merged(Patch::Set(patch)), Metadata::from([("a", "9"), ("c", "3")]));
        assert_eq!(stored.clone().merged(Patch::Keep), stored);
        assert!(stored.merged(Patch::Clear).is_empty());
    }
}
