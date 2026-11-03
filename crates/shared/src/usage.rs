//! Token usage — session-level shape (live fixture):
//! `{"cache_creation":null,"cache_read_input_tokens":0,"input_tokens":0,"output_tokens":0}`
//! Note `cache_creation` is present-and-null, not omitted.

use serde::{Deserialize, Serialize};

use crate::{ApiError, MonetaryAmount};

/// Session/turn token usage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Cache-creation detail (null unless populated) — always serialized.
    pub cache_creation: Option<serde_json::Value>,
    /// Tokens read from cache.
    pub cache_read_input_tokens: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// The cost at list prices so far; absent until it is tracked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_cost: Option<MonetaryAmount>,
}

impl Usage {
    /// Accumulate another usage record into this one.
    pub fn add(&mut self, other: &Usage) {
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }

    /// Take the list cost the model caller reports: the larger of it and the
    /// one recorded, so a stale report never lowers it; `None` means a model
    /// with no list price ran, so the cost is not measurable.
    pub fn record_list_cost(&mut self, reported: Option<MonetaryAmount>) -> Result<(), ApiError> {
        let Some(reported) = reported else {
            self.list_cost = None;
            return Ok(());
        };
        let minor = reported.minor_units("usage.list_cost")?;
        let recorded = self.list_cost.as_ref().and_then(|c| c.minor_units("usage.list_cost").ok());
        if recorded.is_none_or(|r| minor > r) {
            self.list_cost = Some(reported);
        }
        Ok(())
    }

    /// Whether models ran but their cost is not measurable.
    #[must_use]
    pub fn cost_unmeasured(&self) -> bool {
        let tokens = self.input_tokens + self.output_tokens + self.cache_read_input_tokens;
        self.list_cost.is_none() && tokens > 0
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
        a.add(&Usage { input_tokens: 1, output_tokens: 2, cache_read_input_tokens: 3, ..Default::default() });
        assert_eq!(a.input_tokens, 11);
        assert_eq!(a.output_tokens, 7);
        assert_eq!(a.cache_read_input_tokens, 3);
    }

    #[test]
    fn list_cost_keeps_the_larger_report_and_clears_when_unmeasurable() {
        let mut u = Usage { input_tokens: 1, ..Default::default() };
        assert!(u.cost_unmeasured());
        u.record_list_cost(Some(MonetaryAmount::usd(12))).unwrap();
        u.record_list_cost(Some(MonetaryAmount::usd(6))).unwrap();
        assert_eq!(u.list_cost, Some(MonetaryAmount::usd(12)));
        assert!(serde_json::to_string(&u)
            .unwrap()
            .ends_with(r#""list_cost":{"amount":"12","currency":"USD"}}"#));
        u.record_list_cost(None).unwrap();
        assert!(u.cost_unmeasured());
    }
}
