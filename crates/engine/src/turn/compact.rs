//! Keeping a thread's prompt inside its model's window, in tiers (D31):
//! every tool result is capped as it reaches the prompt (CP-3); past 80% of
//! the window, a compaction step summarises the thread into a
//! checkpoint the later prompts start from (CP-4 to CP-12).

pub mod checkpoint;
pub mod estimate;
pub mod prompt;
pub mod summary;

/// The compaction settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    /// `AGENTD_TOOL_OUTPUT_TOKEN_LIMIT`: the most of one tool result a prompt
    /// carries (16,000).
    pub tool_output_limit: u64,
}

impl Default for Compaction {
    fn default() -> Self {
        Self { tool_output_limit: 16_000 }
    }
}

/// What a thread's prompt needs at its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// It fits: results are only capped.
    Whole,
    /// The thread compacts before its next turn.
    Compact,
}

impl Compaction {
    /// The tier of a prompt of `estimate` tokens for a `window`-token model.
    pub fn tier(&self, estimate: u64, window: u64) -> Tier {
        match estimate.saturating_mul(10) >= window.saturating_mul(8) {
            true => Tier::Compact,
            false => Tier::Whole,
        }
    }
}

/// One token per this many bytes, for text no call has counted (CP-1).
pub const BYTES_PER_TOKEN: u64 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_compacts_at_80_percent_of_the_window() {
        let c = Compaction::default();
        assert_eq!(c.tier(10_000, 200_000), Tier::Whole);
        assert_eq!(c.tier(159_999, 200_000), Tier::Whole);
        assert_eq!(c.tier(160_000, 200_000), Tier::Compact);
    }
}
