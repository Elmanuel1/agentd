//! Every tool result capped as it reaches the prompt (CP-3). The stored
//! events stay whole.

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
    fn a_long_result_is_cut_at_the_limit_and_marked() {
        let mut messages = vec![result("sevt_1", &"x".repeat(1000), false)];
        cap(&mut messages, 10);
        assert_eq!(text_of(&messages[0]), format!("{}\n[tool result cut at 10 tokens]", "x".repeat(40)));
    }
}
