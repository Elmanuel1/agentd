//! The prompt-side tiers: every tool result capped as it reaches the prompt
//! (CP-3), and past the clear threshold the older ones replaced by a
//! placeholder naming their call's event (CP-2). The stored events stay
//! whole; tool inputs and error results are never cleared.

use super::BYTES_PER_TOKEN;
use crate::model::{Block, Message};

/// Cap each tool result at `limit` tokens, marking the cut.
pub fn cap(messages: &mut [Message], limit: u64) {
    let max = usize::try_from(limit.saturating_mul(BYTES_PER_TOKEN)).unwrap_or(usize::MAX);
    for block in messages.iter_mut().flat_map(|m| m.content.iter_mut()) {
        let Block::ToolResult { content, .. } = block else { continue };
        if content.len() <= max {
            continue;
        }
        let mut cut = max;
        while !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        content.push_str(&format!("\n[tool result cut at {limit} tokens]"));
    }
}

/// Replace every successful tool result but the newest `keep` with a
/// placeholder naming its call's event.
pub fn clear(messages: &mut [Message], keep: usize) {
    let results = messages.iter_mut().rev().flat_map(|m| m.content.iter_mut().rev());
    let mut kept = 0;
    for block in results {
        let Block::ToolResult { tool_use_id, content, is_error: false } = block else { continue };
        if kept < keep {
            kept += 1;
            continue;
        }
        *content = format!("[tool result cleared: event {tool_use_id}]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;

    fn result(id: &str, text: &str, is_error: bool) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::ToolResult { tool_use_id: id.into(), content: text.into(), is_error }],
        }
    }

    fn text_of(m: &Message) -> &str {
        match &m.content[0] {
            Block::ToolResult { content, .. } => content,
            _ => "",
        }
    }

    #[test]
    fn older_results_are_cleared_and_errors_and_the_newest_kept() {
        let mut messages: Vec<Message> =
            (0..6).map(|i| result(&format!("sevt_{i}"), "out", i == 1)).collect();
        clear(&mut messages, 3);
        let got: Vec<&str> = messages.iter().map(text_of).collect();
        assert_eq!(
            got,
            [
                "[tool result cleared: event sevt_0]",
                "out",
                "[tool result cleared: event sevt_2]",
                "out",
                "out",
                "out"
            ]
        );
    }

    #[test]
    fn a_long_result_is_cut_at_the_limit_and_marked() {
        let mut messages = vec![result("sevt_1", &"x".repeat(1000), false)];
        cap(&mut messages, 10);
        assert_eq!(text_of(&messages[0]), format!("{}\n[tool result cut at 10 tokens]", "x".repeat(40)));
    }
}
