//! A thread's prompt size with no token-counting call (CP-1): the last
//! model call's tokens, plus a token per 4 bytes of what came after it.

use sessions::events::{Event, EventPayload, SpanEvent};

use super::BYTES_PER_TOKEN;
use super::checkpoint::View;

fn bytes(events: &[&Event]) -> u64 {
    let n: usize = events.iter().map(|e| serde_json::to_string(e).map_or(0, |s| s.len())).sum();
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// The estimated prompt of `view`, in tokens.
pub fn estimate(view: &View<'_>) -> u64 {
    let last = view.since.iter().rposition(|e| {
        matches!(e.payload, EventPayload::Span(SpanEvent::ModelRequestEnd { is_error: false, .. }))
    });
    match last.and_then(|i| Some((view.since.get(i)?, view.since.get(i + 1..).unwrap_or_default()))) {
        Some((end, after)) => {
            let EventPayload::Span(SpanEvent::ModelRequestEnd { model_usage, .. }) = &end.payload else {
                return 0;
            };
            let t = &model_usage.tokens;
            t.input_tokens
                + t.cache_read_input_tokens
                + t.cache_creation_input_tokens
                + t.output_tokens
                + bytes(after) / BYTES_PER_TOKEN
        }
        None => {
            let summary = view.checkpoint.as_ref().map_or(0, |c| c.bytes());
            (bytes(&view.prompt) + summary) / BYTES_PER_TOKEN
        }
    }
}
