//! Keeping a thread's prompt inside its model's window, in tiers (D31):
//! every tool result is capped as it reaches the prompt (CP-3); past
//! `clear_at` or half the window, old tool results are cleared (CP-2); past
//! 80% of the window, a compaction step summarises the thread into a
//! checkpoint the later prompts start from (CP-4 to CP-12).

pub mod checkpoint;
pub mod estimate;
pub mod prompt;
pub mod summary;

/// The compaction settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    /// `AGENTD_COMPACTION_CLEAR_AT`: the prompt size, in tokens, past which
    /// old tool results are cleared (100,000).
    pub clear_at: u64,
    /// `AGENTD_COMPACTION_KEEP_RESULTS`: the newest tool results kept whole
    /// when clearing (3, from 3 to 5).
    pub keep_results: usize,
    /// `AGENTD_TOOL_OUTPUT_TOKEN_LIMIT`: the most of one tool result a prompt
    /// carries (16,000).
    pub tool_output_limit: u64,
}

impl Default for Compaction {
    fn default() -> Self {
        Self { clear_at: 100_000, keep_results: 3, tool_output_limit: 16_000 }
    }
}

/// What a thread's prompt needs at its size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// It fits: results are only capped.
    Whole,
    /// Old tool results are cleared.
    Clear,
    /// The thread compacts before its next turn.
    Compact,
}

impl Compaction {
    /// The tier of a prompt of `estimate` tokens for a `window`-token model.
    pub fn tier(&self, estimate: u64, window: u64) -> Tier {
        if estimate.saturating_mul(10) >= window.saturating_mul(8) {
            Tier::Compact
        } else if estimate >= self.clear_at.min(window / 2) {
            Tier::Clear
        } else {
            Tier::Whole
        }
    }
}

/// One token per this many bytes, for text no call has counted (CP-1).
pub const BYTES_PER_TOKEN: u64 = 4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tier_follows_the_window_and_the_clear_setting() {
        let c = Compaction::default();
        assert_eq!(c.tier(10_000, 200_000), Tier::Whole);
        assert_eq!(c.tier(100_000, 200_000), Tier::Clear, "clear_at is the lower");
        assert_eq!(c.tier(60_000, 100_000), Tier::Clear, "half the window is the lower");
        assert_eq!(c.tier(160_000, 200_000), Tier::Compact);
    }
}
