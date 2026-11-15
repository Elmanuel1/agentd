//! A compaction call: what it summarises, its instructions, and the
//! checkpoint its reply makes (CP-6 to CP-8). It summarises from the
//! latest checkpoint, keeps the thread's last two model calls verbatim
//! after the summary, and quotes the user's messages newest first.

use sessions::events::{Event, EventId, EventPayload, SpanEvent, UserEvent};
use shared::ContentBlock;

use super::checkpoint::Checkpoint;
use super::BYTES_PER_TOKEN;
use crate::model::{Block, ModelResponse};

/// What the summary must carry.
const ASK: &str = "Summarize the conversation so far for the model that will continue it. Keep the goal, \
the constraints, the decisions made, the files touched, the open todos, the unresolved errors and the next \
steps. Do not call tools; answer with the summary text only.";
/// The handoff prompt for a model with no provider-side compaction (C5).
const HANDOFF: &str = "Create a handoff summary for another LLM that will resume the task.";
/// The user's messages a summary quotes, newest first, at most.
const USER_TOKENS: usize = 20_000;
/// The longest `instructions` the Messages API takes (C1).
pub const ANTHROPIC_LIMIT: usize = 16_384;
/// The model calls kept verbatim after the summary.
const KEPT_CALLS: usize = 2;

/// Where a compaction of `prompt` cuts: it summarises the events before
/// the second-to-last model call; `None` when there is nothing before them.
pub fn cut(prompt: &[&Event]) -> Option<usize> {
    let starts: Vec<usize> = prompt
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.payload, EventPayload::Span(SpanEvent::ModelRequestStart {})))
        .map(|(i, _)| i)
        .collect();
    let at = *starts.len().checked_sub(KEPT_CALLS).and_then(|i| starts.get(i))?;
    (at > 0).then_some(at)
}

/// The instructions of a compaction of `prompt`: what to keep, the handoff
/// prompt where the provider has no compaction, and the user's messages
/// verbatim newest first, the one that overflows cut; at most `limit`
/// characters.
pub fn instructions(prompt: &[&Event], anthropic: bool) -> String {
    let mut out = match anthropic {
        true => ASK.to_owned(),
        false => format!("{HANDOFF} {ASK}"),
    };
    let limit = match anthropic {
        true => ANTHROPIC_LIMIT,
        false => USER_TOKENS * 4 + out.len() + 64,
    };
    let mut budget = (USER_TOKENS * usize::try_from(BYTES_PER_TOKEN).unwrap_or(4))
        .min(limit.saturating_sub(out.len() + 64));
    let users = prompt.iter().rev().filter_map(|e| match &e.payload {
        EventPayload::User(UserEvent::Message { content }) => {
            Some(content.iter().filter_map(ContentBlock::as_text).collect::<Vec<_>>().join("\n"))
        }
        _ => None,
    });
    let mut quoted = Vec::new();
    for text in users {
        if budget == 0 {
            break;
        }
        let mut end = text.len().min(budget);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        budget -= end;
        quoted.push(text.get(..end).unwrap_or_default().to_owned());
    }
    if !quoted.is_empty() {
        out.push_str("\n\nThe user's messages, newest first, verbatim:\n\n");
        out.push_str(&quoted.join("\n---\n"));
    }
    out.truncate(limit);
    out
}

/// The checkpoint `reply` makes, covering up to `through`; `None` when it
/// carries no summary (CP-10).
pub fn checkpoint(
    reply: &ModelResponse,
    model: &str,
    anthropic: bool,
    through: EventId,
) -> Option<Checkpoint> {
    let (block, summary) = match anthropic {
        true => (reply.content.iter().find(|b| matches!(b, Block::Compaction { .. })).cloned(), None),
        false => {
            let text: Vec<&str> = reply
                .content
                .iter()
                .filter_map(|b| match b {
                    Block::Text { text } if !text.is_empty() => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            (None, (!text.is_empty()).then(|| text.join("\n")))
        }
    };
    (block.is_some() || summary.is_some()).then(|| Checkpoint {
        through,
        model: model.to_owned(),
        block,
        summary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Role, StopKind};

    #[test]
    fn a_model_without_provider_compaction_hands_off_and_its_summary_leads_the_prompt() {
        // CP-8.
        let through = EventId::generate();
        let reply = ModelResponse::text("the goal is X");
        let cp = checkpoint(&reply, "openai/gpt-x", false, through.clone()).unwrap();
        assert_eq!((cp.summary.as_deref(), cp.block.as_ref()), (Some("the goal is X"), None));
        let m = cp.message();
        assert_eq!(m.role, Role::User);
        assert_eq!(m.content, [Block::text("A summary of the conversation so far:\n\nthe goal is X")]);
        assert!(instructions(&[], false).starts_with(HANDOFF));
        let empty = ModelResponse { content: Vec::new(), stop: StopKind::EndTurn, ..ModelResponse::text("") };
        assert!(checkpoint(&empty, "claude-haiku-4-5", true, through).is_none(), "no summary, no checkpoint");
    }

    #[test]
    fn the_instructions_quote_the_user_newest_first_within_the_limit() {
        let msg = |t: &str| {
            Event::new(EventPayload::User(UserEvent::Message { content: vec![ContentBlock::text(t)] }))
        };
        let (old, new) = (msg("first ask"), msg(&"y".repeat(20_000)));
        let text = instructions(&[&old, &new], true);
        assert!(text.len() <= ANTHROPIC_LIMIT);
        assert!(text.contains("yyyy") && !text.contains("first ask"), "the newest fills the room first");
        let short = instructions(&[&old, &msg("second ask")], true);
        assert!(short.find("second ask") < short.find("first ask"));
    }
}
