//! Anthropic's list prices per model, as
//! <https://platform.claude.com/docs/en/about-claude/pricing> states them on
//! 2026-11-20. Every request agentd sends uses the 5-minute cache, the
//! global inference region and standard speed, so those are the only rates
//! kept.

/// A model's list rates, in thousandths of a US dollar per million tokens:
/// one token at a rate of `r` costs `r` billionths of a dollar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Price {
    /// Uncached input.
    pub input: u64,
    /// A 5-minute cache write.
    pub cache_write: u64,
    /// A cache hit or refresh.
    pub cache_read: u64,
    /// Output.
    pub output: u64,
}

const fn rates(input: u64, cache_write: u64, cache_read: u64, output: u64) -> Price {
    Price { input, cache_write, cache_read, output }
}

const FABLE_5_2: Price = rates(8_000, 10_000, 200, 40_000);
const FABLE_5_1: Price = rates(10_000, 12_500, 250, 50_000);
const FABLE_5: Price = rates(10_000, 12_500, 1_000, 50_000);
const OPUS_5_5: Price = rates(4_000, 5_000, 200, 20_000);
const OPUS_5: Price = rates(5_000, 6_250, 500, 25_000);
const OPUS_4_1: Price = rates(15_000, 18_750, 1_500, 75_000);
const SONNET_5_5: Price = rates(2_000, 2_500, 100, 10_000);
const SONNET_5: Price = rates(2_000, 2_500, 200, 10_000);
const SONNET_4_6: Price = rates(3_000, 3_750, 300, 15_000);
const HAIKU_5_5: Price = rates(100, 125, 10, 500);
const HAIKU_5_5_LONG: Price = rates(500, 625, 50, 2_500);
const HAIKU_4_5: Price = rates(1_000, 1_250, 100, 5_000);

/// Claude Haiku 5.5 charges its long rates for a prompt over this many
/// tokens, cache reads and writes included.
const HAIKU_5_5_LONG_PROMPT: u64 = 100_000;

/// Each priced model id, without its snapshot date.
const LISTED: [(&str, Price); 21] = [
    ("claude-fable-5-2", FABLE_5_2),
    ("claude-mythos-5-2", FABLE_5_2),
    ("claude-fable-5-1", FABLE_5_1),
    ("claude-mythos-5-1", FABLE_5_1),
    ("claude-fable-5", FABLE_5),
    ("claude-mythos-5", FABLE_5),
    ("claude-opus-5-5", OPUS_5_5),
    ("claude-opus-5", OPUS_5),
    ("claude-opus-4-8", OPUS_5),
    ("claude-opus-4-7", OPUS_5),
    ("claude-opus-4-6", OPUS_5),
    ("claude-opus-4-5", OPUS_5),
    ("claude-opus-4-1", OPUS_4_1),
    ("claude-opus-4", OPUS_4_1),
    ("claude-sonnet-5-5", SONNET_5_5),
    ("claude-sonnet-5", SONNET_5),
    ("claude-sonnet-4-6", SONNET_4_6),
    ("claude-sonnet-4-5", SONNET_4_6),
    ("claude-sonnet-4", SONNET_4_6),
    ("claude-haiku-5-5", HAIKU_5_5),
    ("claude-haiku-4-5", HAIKU_4_5),
];

/// `model` without a trailing `-YYYYMMDD` snapshot date.
fn undated(model: &str) -> &str {
    match model.rsplit_once('-') {
        Some((base, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => model,
    }
}

/// The list rates of `model`, an id or its dated snapshot, for a prompt of
/// `prompt_tokens`; `None` when Anthropic lists no price for it.
#[must_use]
pub fn list_price(model: &str, prompt_tokens: u64) -> Option<Price> {
    let id = undated(model);
    let (_, price) = LISTED.iter().find(|(listed, _)| *listed == id)?;
    match id == "claude-haiku-5-5" && prompt_tokens > HAIKU_5_5_LONG_PROMPT {
        true => Some(HAIKU_5_5_LONG),
        false => Some(*price),
    }
}

/// A budgeted session can run `model`: a `vendor/model` id's provider
/// reports each request's cost, and any other id goes to the Messages API
/// at its list price.
#[must_use]
pub fn budgetable(model: &str) -> bool {
    model.contains('/') || list_price(model, 0).is_some()
}

impl Price {
    /// What the tokens cost, in billionths of a US dollar.
    #[must_use]
    pub fn nanos(&self, input: u64, cache_write: u64, cache_read: u64, output: u64) -> u64 {
        input * self.input
            + cache_write * self.cache_write
            + cache_read * self.cache_read
            + output * self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dated_snapshot_takes_its_model_s_price() {
        assert_eq!(list_price("claude-haiku-4-5-20251001", 0), Some(HAIKU_4_5));
        assert_eq!(list_price("claude-haiku-4-5", 0), Some(HAIKU_4_5));
        assert_eq!(list_price("claude-opus-4-20250514", 0), Some(OPUS_4_1));
        assert_eq!(list_price("claude-opus-4-5-20251101", 0), Some(OPUS_5));
        assert_eq!(list_price("claude-sonnet-5", 0), Some(SONNET_5));
        assert_eq!(list_price("claude-sonnet-5-5", 0), Some(SONNET_5_5));
    }

    #[test]
    fn an_unlisted_model_has_no_price() {
        assert_eq!(list_price("claude-mythos-preview", 0), None);
        assert_eq!(list_price("claude-3-haiku-20240307", 0), None);
        assert_eq!(list_price("claude-haiku-4-5-2025", 0), None);
        assert!(!budgetable("claude-mythos-preview"));
        assert!(budgetable("openai/gpt-6-luna"));
        assert!(budgetable("claude-haiku-4-5"));
    }

    #[test]
    fn haiku_5_5_charges_its_long_rates_over_100k_prompt_tokens() {
        assert_eq!(list_price("claude-haiku-5-5", 100_000), Some(HAIKU_5_5));
        assert_eq!(list_price("claude-haiku-5-5", 100_001), Some(HAIKU_5_5_LONG));
        assert_eq!(list_price("claude-haiku-4-5", 1_000_000), Some(HAIKU_4_5));
    }

    #[test]
    fn a_million_tokens_cost_the_listed_rate() {
        // Haiku 4.5: $1 input, $1.25 cache write, $0.10 cache read, $5 output per MTok.
        let m = 1_000_000;
        assert_eq!(HAIKU_4_5.nanos(m, 0, 0, 0), 1_000_000_000);
        assert_eq!(HAIKU_4_5.nanos(0, m, 0, 0), 1_250_000_000);
        assert_eq!(HAIKU_4_5.nanos(0, 0, m, 0), 100_000_000);
        assert_eq!(HAIKU_4_5.nanos(0, 0, 0, m), 5_000_000_000);
    }
}
