//! What a model call costs the session, and its budget (L-16, L-18).

use serde_json::Value;
use sessions::steps::Claim;
use sessions::types::Session;
use shared::BackendError;
use shared::{MonetaryAmount, Usage};

use super::Loop;
use crate::model::budget::Admit;

use crate::model::{Block, ModelResponse};
use crate::turn::history::Kept;

/// Billionths of a US dollar in a cent.
const NANOS_PER_CENT: u64 = 10_000_000;
/// What the loop keeps beside a call's `span.model_request_end`: its exact
/// list cost in billionths of a dollar.
const KEPT: &str = "list_cost_nanos";

/// A call's exact list cost in billionths of a dollar; `None` when its model
/// has no price.
pub(super) fn nanos(r: &ModelResponse) -> Option<u64> {
    r.cost.filter(|c| c.is_finite() && *c >= 0.0).map(|c| (c * 1e9).round() as u64)
}

/// The usage one call adds, and its exact list cost.
pub(super) fn spent(r: &ModelResponse) -> (Usage, Option<u64>) {
    let usage = Usage {
        cache_read_input_tokens: r.usage.cache_read_input_tokens,
        input_tokens: r.usage.input_tokens + r.usage.cache_creation_input_tokens,
        output_tokens: r.usage.output_tokens,
        ..Usage::default()
    };
    (usage, nanos(r))
}

/// The value kept beside a call's `span.model_request_end`.
pub(super) fn kept(nanos: u64) -> Value {
    Value::Object([(KEPT.to_owned(), nanos.into())].into_iter().collect())
}

/// The session's exact list cost so far, from what the log keeps beside its calls.
pub(super) fn so_far(kept: &Kept) -> u64 {
    kept.values().filter_map(|v| v.get(KEPT)?.as_u64()).sum()
}

/// `session` after a call that used `usage` and cost `cost`, on top of the
/// exact `before`. As on live, `usage.list_cost` is rounded to the nearest cent.
pub(super) fn charged(session: &mut Session, usage: &Usage, cost: Option<u64>, before: u64) {
    let measured = !session.usage.cost_unmeasured();
    session.usage.add(usage);
    session.usage.list_cost = match (measured, cost) {
        (true, Some(c)) => Some(MonetaryAmount::usd((before + c + NANOS_PER_CENT / 2) / NANOS_PER_CENT)),
        _ => None,
    };
}

/// The session's budget is spent: its exact list cost so far, `spent`,
/// reached it, or a model with no list price ran.
pub(super) fn over_budget(session: &Session, spent: u64) -> bool {
    let Some(max) = session.budget.as_ref().and_then(|b| b.max_minor_units().ok()) else {
        return false;
    };
    session.usage.cost_unmeasured() || spent >= max.saturating_mul(NANOS_PER_CENT)
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

impl Loop {
    /// The account-wide budget lets `claim`'s step call `provider` now (M-4);
    /// otherwise the step is deferred, to the next minute or, with the budget
    /// unreadable, for a second.
    pub(super) async fn admitted(&self, claim: &Claim, provider: &str) -> Result<bool, BackendError> {
        let wait = match self.models.budget.admit(provider).await {
            Ok(Admit::Yes) => return Ok(true),
            Ok(Admit::Later(seconds)) => seconds,
            Err(e) => {
                tracing::warn!(session = %claim.session, error = %e, "loop: the model budget cannot be read; the step waits");
                1
            }
        };
        self.stores.steps.defer(claim, wait).await.map_err(BackendError::new)?;
        Ok(false)
    }
}
