//! A thread's latest checkpoint and the events its prompt reads after it.
//! The checkpoint is an `agent.thread_context_compacted` event; what it
//! holds (the summary, the last event it covers, the model that wrote it)
//! is kept beside the event, which the API serves with live's fields only.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sessions::events::{AgentEvent, Event, EventId, EventPayload, SpanEvent};

use crate::model::{Block, Message, Role};
use crate::turn::history::Kept;

/// A compaction's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// The last event the summary covers.
    pub through: EventId,
    /// That event's `seq`: one checkpoint per thread and covered event (V11).
    pub through_seq: i64,
    /// The model that wrote it.
    pub model: String,
    /// The Messages API's signed block, for an Anthropic model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block: Option<Block>,
    /// The summary text, for any other model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

impl Checkpoint {
    /// What the loop keeps beside the checkpoint event.
    pub fn kept(&self) -> Value {
        serde_json::json!({ "checkpoint": self })
    }

    /// The message a later prompt starts with: the signed block as it came,
    /// or the summary as the user's text.
    pub fn message(&self) -> Message {
        match (&self.block, &self.summary) {
            (Some(block), _) => Message { role: Role::Assistant, content: vec![block.clone()] },
            (None, summary) => Message {
                role: Role::User,
                content: vec![Block::text(format!(
                    "A summary of the conversation so far:\n\n{}",
                    summary.as_deref().unwrap_or_default()
                ))],
            },
        }
    }

    /// Its size, in the estimate's tokens.
    pub fn bytes(&self) -> u64 {
        let text = match (&self.block, &self.summary) {
            (Some(Block::Compaction { content, .. }), _) => content.len(),
            (_, Some(s)) => s.len(),
            _ => 0,
        };
        u64::try_from(text).unwrap_or(u64::MAX)
    }
}

/// The marker kept beside a compaction call's `span.model_request_start`.
pub fn attempt() -> Value {
    serde_json::json!({ "compaction": true })
}

/// A thread's events as its prompt reads them.
#[derive(Debug, Default)]
pub struct View<'a> {
    /// The latest checkpoint.
    pub checkpoint: Option<Checkpoint>,
    /// The events after the last one it covers, compaction calls left out.
    pub prompt: Vec<&'a Event>,
    /// The events written after the checkpoint itself, whose calls' usage
    /// counts only the prompt since it.
    pub since: Vec<&'a Event>,
    /// The thread's latest model call was a compaction that left no
    /// checkpoint: the next call is the turn's (CP-10).
    pub attempted: bool,
}

/// `events` (one thread's, oldest first) read from its latest checkpoint.
pub fn view<'a>(events: &[&'a Event], kept: &Kept) -> View<'a> {
    let flagged =
        |id: &EventId| kept.get(id).is_some_and(|k| k.get("compaction") == Some(&Value::Bool(true)));
    let compaction = |e: &Event| match &e.payload {
        EventPayload::Span(SpanEvent::ModelRequestStart {}) => flagged(&e.id),
        EventPayload::Span(SpanEvent::ModelRequestEnd { model_request_start_id, .. }) => {
            flagged(model_request_start_id)
        }
        EventPayload::Agent(AgentEvent::ThreadContextCompacted {}) => true,
        _ => false,
    };
    let latest = events.iter().enumerate().rev().find_map(|(i, e)| {
        let is = matches!(e.payload, EventPayload::Agent(AgentEvent::ThreadContextCompacted {}));
        let cp: Checkpoint = serde_json::from_value(kept.get(&e.id)?.get("checkpoint")?.clone()).ok()?;
        is.then_some((i, cp))
    });
    let last_start = events
        .iter()
        .rev()
        .find(|e| matches!(e.payload, EventPayload::Span(SpanEvent::ModelRequestStart {})));
    let attempted = last_start.is_some_and(|s| {
        let at = events.iter().position(|e| e.id == s.id).unwrap_or_default();
        flagged(&s.id) && latest.as_ref().is_none_or(|(i, _)| *i < at)
    });
    let Some((at, cp)) = latest else {
        let prompt = events.iter().copied().filter(|e| !compaction(e)).collect::<Vec<_>>();
        return View { checkpoint: None, since: prompt.clone(), prompt, attempted };
    };
    let through = events.iter().position(|e| e.id == cp.through).unwrap_or(at);
    let after = |from: usize| events.iter().skip(from + 1).copied().filter(|e| !compaction(e)).collect();
    View { prompt: after(through), since: after(at), checkpoint: Some(cp), attempted }
}
