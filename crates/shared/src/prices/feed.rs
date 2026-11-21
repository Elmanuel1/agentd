//! Anthropic's list prices as OpenRouter's public model list
//! (`GET https://openrouter.ai/api/v1/models`) states them: each
//! `anthropic/<model>` entry's `pricing`, in US dollars per token as decimal
//! strings, with `overrides` for long prompts. Its rates are Anthropic's own.

use std::collections::HashMap;

use serde_json::Value;

use super::Price;

/// A model's rates, and the rates it charges for a prompt over each
/// threshold of tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tiers {
    base: Price,
    over: Vec<(u64, Price)>,
}

/// The rates the list states, by Anthropic model id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed(HashMap<String, Tiers>);

impl Feed {
    /// The `anthropic/` models of a model list. An entry with a variant
    /// (`:batch`) or a rate that is missing or not a plain decimal is left
    /// out, so the built-in table prices that model.
    #[must_use]
    pub fn parse(list: &Value) -> Self {
        let entries = list.get("data").and_then(Value::as_array).into_iter().flatten();
        Self(entries.filter_map(|m| Some((model_id(m)?, tiers(m.get("pricing")?)?))).collect())
    }

    /// Whether the list priced no Anthropic model.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `model`'s rates for a prompt of `prompt_tokens`, when the list has it.
    pub(super) fn price(&self, model: &str, prompt_tokens: u64) -> Option<Price> {
        let t = self.0.get(model)?;
        let over = t.over.iter().filter(|(min, _)| prompt_tokens > *min).max_by_key(|(min, _)| *min);
        Some(over.map_or(t.base, |(_, p)| *p))
    }
}

/// `anthropic/claude-haiku-4.5` is Anthropic's `claude-haiku-4-5`.
fn model_id(m: &Value) -> Option<String> {
    let id = m.get("id")?.as_str()?.strip_prefix("anthropic/")?;
    (!id.contains(':')).then(|| id.replace('.', "-"))
}

fn tiers(pricing: &Value) -> Option<Tiers> {
    let overrides = pricing.get("overrides").and_then(Value::as_array).cloned().unwrap_or_default();
    let over = overrides
        .iter()
        .map(|o| Some((o.get("min_prompt_tokens")?.as_u64()?, price(o)?)))
        .collect::<Option<Vec<_>>>()?;
    Some(Tiers { base: price(pricing)?, over })
}

fn price(p: &Value) -> Option<Price> {
    let rate = |field: &str| nanos(p.get(field)?.as_str()?);
    Some(Price {
        input: rate("prompt")?,
        cache_write: rate("input_cache_write")?,
        cache_read: rate("input_cache_read")?,
        output: rate("completion")?,
    })
}

/// A rate in dollars per token as billionths of a dollar per token, which is
/// thousandths of a dollar per million tokens; past nine decimals it rounds.
fn nanos(dollars: &str) -> Option<u64> {
    let (whole, fraction) = dollars.split_once('.').unwrap_or((dollars, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !digits(whole) || !digits(fraction) {
        return None;
    }
    let kept: String = fraction.chars().chain(std::iter::repeat('0')).take(9).collect();
    let up = u64::from(fraction.as_bytes().get(9).is_some_and(|d| *d >= b'5'));
    whole.parse::<u64>().ok()?.checked_mul(1_000_000_000)?.checked_add(kept.parse::<u64>().ok()? + up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Entries as the list served them on 2026-11-21.
    fn list() -> Value {
        json!({"data": [
            {"id": "anthropic/claude-haiku-4.5", "pricing": {"prompt": "0.000001", "completion": "0.000005",
                "web_search": "0.01", "input_cache_read": "0.0000001", "input_cache_write": "0.00000125",
                "input_cache_write_1h": "0.000002"}},
            {"id": "anthropic/claude-haiku-4.5:batch", "pricing": {"prompt": "0.0000005", "completion": "0.0000025",
                "input_cache_read": "0.00000005", "input_cache_write": "0.000000625"}},
            {"id": "anthropic/claude-sonnet-4.5", "pricing": {"prompt": "0.000003", "completion": "0.000015",
                "input_cache_read": "0.0000003", "input_cache_write": "0.00000375", "input_cache_write_1h": "0.000006",
                "overrides": [{"min_prompt_tokens": 200000, "prompt": "0.000006", "completion": "0.0000225",
                    "input_cache_read": "0.0000006", "input_cache_write": "0.0000075",
                    "input_cache_write_1h": "0.000012"}]}},
            {"id": "anthropic/claude-opus-5", "pricing": {"prompt": "0.000005", "completion": "0.000025"}},
            {"id": "openai/gpt-6-luna", "pricing": {"prompt": "0.000001", "completion": "0.000002",
                "input_cache_read": "0", "input_cache_write": "0"}},
        ]})
    }

    #[test]
    fn an_anthropic_entry_is_its_model_s_rates_in_billionths_of_a_dollar_per_token() {
        let feed = Feed::parse(&list());
        let haiku = Price { input: 1_000, cache_write: 1_250, cache_read: 100, output: 5_000 };
        assert_eq!(feed.price("claude-haiku-4-5", 0), Some(haiku));
        assert_eq!(
            feed.0.len(),
            2,
            "the batch variant, a model with missing rates and other vendors are left out"
        );
        assert_eq!(feed.price("claude-opus-5", 0), None);
    }

    #[test]
    fn a_prompt_over_an_override_s_threshold_takes_its_rates() {
        let feed = Feed::parse(&list());
        let long = Price { input: 6_000, cache_write: 7_500, cache_read: 600, output: 22_500 };
        assert_eq!(feed.price("claude-sonnet-4-5", 200_000).map(|p| p.input), Some(3_000));
        assert_eq!(feed.price("claude-sonnet-4-5", 200_001), Some(long));
    }

    #[test]
    fn a_rate_is_read_exactly_from_its_decimal() {
        assert_eq!(nanos("0.000000125"), Some(125));
        assert_eq!(nanos("0.00000000625"), Some(6));
        assert_eq!(nanos("0.0000000625"), Some(63));
        assert_eq!(nanos("0.01"), Some(10_000_000));
        assert_eq!(nanos("2"), Some(2_000_000_000));
        assert_eq!(nanos("1e-6"), None);
        assert_eq!(nanos("-0.1"), None);
        assert_eq!(nanos(".5"), None);
    }
}
