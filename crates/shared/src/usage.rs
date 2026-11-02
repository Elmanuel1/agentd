//! Token usage — session-level shape (live fixture):
//! `{"cache_creation":null,"cache_read_input_tokens":0,"input_tokens":0,"output_tokens":0}`
//! Note `cache_creation` is present-and-null, not omitted.

use serde::{Deserialize, Serialize};

/// Session/turn token usage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Cache-creation detail (null unless populated) — always serialized.
    pub cache_creation: Option<serde_json::Value>,
    /// Tokens read from cache.
    pub cache_read_input_tokens: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
}

impl Usage {
    /// Accumulate another usage record into this one.
    pub fn add(&mut self, other: &Usage) {
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_usage_matches_live_fixture() {
        let u = Usage::default();
        assert_eq!(
            serde_json::to_string(&u).unwrap(),
            r#"{"cache_creation":null,"cache_read_input_tokens":0,"input_tokens":0,"output_tokens":0}"#
        );
    }

    #[test]
    fn accumulation() {
        let mut a = Usage { input_tokens: 10, output_tokens: 5, ..Default::default() };
        a.add(&Usage { input_tokens: 1, output_tokens: 2, cache_read_input_tokens: 3, cache_creation: None });
        assert_eq!(a.input_tokens, 11);
        assert_eq!(a.output_tokens, 7);
        assert_eq!(a.cache_read_input_tokens, 3);
    }
}
