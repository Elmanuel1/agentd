//! What a model call costs the session, and its budget (L-16, L-18).

use sessions::types::Session;
use shared::{MonetaryAmount, Usage};

use crate::model::{Block, ModelResponse};

/// The usage one call adds, and its list cost in cents rounded up, so a
/// budget is never passed by rounding; `None` when the model has no price.
pub(super) fn spent(r: &ModelResponse) -> (Usage, Option<u64>) {
    let usage = Usage {
        cache_read_input_tokens: r.usage.cache_read_input_tokens,
        input_tokens: r.usage.input_tokens + r.usage.cache_creation_input_tokens,
        output_tokens: r.usage.output_tokens,
        ..Usage::default()
    };
    let cents = r.cost.filter(|c| c.is_finite() && *c >= 0.0).map(|c| (c * 100.0).ceil() as u64);
    (usage, cents)
}

/// `session` after a call that used `usage` and cost `cents`.
pub(super) fn charged(session: &mut Session, usage: &Usage, cents: Option<u64>) {
    let measured = !session.usage.cost_unmeasured();
    let before =
        session.usage.list_cost.as_ref().and_then(|c| c.minor_units("usage.list_cost").ok()).unwrap_or(0);
    session.usage.add(usage);
    session.usage.list_cost = match (measured, cents) {
        (true, Some(c)) => Some(MonetaryAmount::usd(before + c)),
        _ => None,
    };
}

/// The session's budget is spent: its list cost reached it, or a model with
/// no list price ran.
pub(super) fn over_budget(session: &Session) -> bool {
    let Some(max) = session.budget.as_ref().and_then(|b| b.max_minor_units().ok()) else {
        return false;
    };
    let cost = session.usage.list_cost.as_ref().and_then(|c| c.minor_units("usage.list_cost").ok());
    session.usage.cost_unmeasured() || cost.is_some_and(|c| c >= max)
}

/// What the model said, its text blocks joined.
pub(super) fn final_text(r: &ModelResponse) -> String {
    let texts: Vec<&str> = r
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    texts.join("\n")
}
