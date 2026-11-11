//! What a model call's reply leads to: its events, in live's order, and how
//! the step ends. Built-in calls are parked for the session's worker; the
//! client answers confirmations and custom tools.

use sessions::events::{AgentEvent, Event, EventId, SpanEvent, StopReason};
use sessions::steps::StepEnd;
use sessions::types::Session;

use super::turn::Turn;
use shared::ContentBlock;

use crate::model::{Block, ModelResponse};
use crate::turn::policy::{Disposition, Toolbox};

/// What the reply leads to.
pub(super) struct Led {
    pub(super) events: Vec<Event>,
    pub(super) end: StepEnd,
    /// The stop reason when the turn idles.
    pub(super) idle: Option<StopReason>,
}

/// One model call, as the reply's events need it.
pub(super) struct Call<'a> {
    pub(super) turn: &'a Turn,
    /// The call's `span.model_request_start`.
    pub(super) start: &'a EventId,
    pub(super) tools: &'a Toolbox,
    /// Keep the session running while its worker runs built-in calls.
    pub(super) cloud: bool,
    /// The client sent events during the call, which the next call takes.
    pub(super) more: bool,
    pub(super) session: &'a Session,
    pub(super) log: &'a [Event],
}

/// The reply's events, and how the step ends.
pub(super) fn led(c: &Call<'_>, r: &ModelResponse) -> Led {
    let (turn, start, tools, cloud, more) = (c.turn, c.start, c.tools, c.cloud, c.more);
    let mut events = Vec::new();
    if r.thought {
        events.push(turn.event(AgentEvent::Thinking { content: None }));
    }
    let text: Vec<ContentBlock> = r
        .content
        .iter()
        .filter_map(|b| match b {
            Block::Text { text } if !text.is_empty() => Some(ContentBlock::text(text.clone())),
            _ => None,
        })
        .collect();
    if !text.is_empty() {
        events.push(turn.event(AgentEvent::Message { content: text }));
    }
    events.push(turn.event(SpanEvent::ModelRequestEnd {
        model_request_start_id: start.clone(),
        is_error: false,
        model_usage: r.usage.clone(),
    }));
    let (mut waiting, mut parked, mut unknown) = (Vec::new(), false, false);
    for (_, name, input) in r.tool_uses() {
        let call = match tools.disposition(name) {
            Some(Disposition::Custom) => {
                turn.event(AgentEvent::CustomToolUse { name: name.to_owned(), input: input.clone() })
            }
            Some(Disposition::Ask) => turn.event(AgentEvent::ToolUse {
                name: name.to_owned(),
                input: input.clone(),
                evaluated_permission: Some("ask".into()),
                evaluation: None,
            }),
            Some(Disposition::Allow { evaluation }) => turn.event(AgentEvent::ToolUse {
                name: name.to_owned(),
                input: input.clone(),
                evaluated_permission: Some("allow".into()),
                evaluation: evaluation.clone(),
            }),
            Some(Disposition::Delegate) => {
                unknown = true;
                events.extend(super::team::delegate(turn, c.session, c.log, name, input));
                continue;
            }
            None => {
                unknown = true;
                let call = turn.event(AgentEvent::ToolUse {
                    name: name.to_owned(),
                    input: input.clone(),
                    evaluated_permission: Some("allow".into()),
                    evaluation: None,
                });
                let result = turn.event(AgentEvent::ToolResult {
                    tool_use_id: call.id.clone(),
                    content: vec![ContentBlock::text(format!("Unknown tool `{name}`."))],
                    is_error: true,
                });
                events.extend([call, result]);
                continue;
            }
        };
        let for_worker = matches!(tools.disposition(name), Some(Disposition::Allow { .. }));
        parked |= for_worker;
        if !for_worker || !cloud {
            waiting.push(call.id.clone());
        }
        events.push(call);
    }
    let end = match (parked, unknown || more) {
        (true, _) => StepEnd::Requeue,
        (false, true) => StepEnd::Again,
        (false, false) => StepEnd::Done,
    };
    let idle = match (waiting.is_empty(), parked || unknown || more) {
        (false, _) => Some(StopReason::RequiresAction { event_ids: waiting }),
        (true, false) => Some(StopReason::EndTurn {}),
        (true, true) => None,
    };
    Led { events, end, idle }
}
