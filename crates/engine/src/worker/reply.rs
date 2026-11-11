//! What a model call's reply leads to: its events, in live's order, and how
//! the step ends. Built-in calls are parked for the session's worker; the
//! client answers confirmations and custom tools.

use sessions::events::{AgentEvent, Event, EventId, EventPayload, SessionEvent, SpanEvent, StopReason};
use sessions::steps::StepEnd;
use sessions::types::{Session, SessionStatus, ThreadId};
use shared::{ContentBlock, MonetaryAmount, Timestamp, Usage};

use crate::model::{Block, ModelResponse};
use crate::turn::policy::{Disposition, Toolbox};

/// Who a step's events belong to.
pub(super) struct Turn {
    /// The thread the call was for.
    pub(super) thread: ThreadId,
    /// The agent that thread runs.
    pub(super) agent: String,
}

impl Turn {
    /// A stamped event of this turn's thread.
    pub(super) fn event(&self, payload: impl Into<EventPayload>) -> Event {
        let mut e = Event::new(payload.into());
        e.processed_at = Some(Timestamp::now());
        e
    }

    fn lifecycle(&self, payload: SessionEvent) -> Event {
        let mut e = self.event(payload);
        e.session_thread_id = Some(self.thread.clone());
        e
    }

    /// The session and its thread start running.
    pub(super) fn running(&self) -> Vec<Event> {
        vec![
            self.event(SessionEvent::StatusRunning {}),
            self.lifecycle(SessionEvent::ThreadStatusRunning { agent_name: self.agent.clone() }),
        ]
    }

    /// The turn ends as `stop` says: the thread, then the session's usage as
    /// `session` holds it after the step, then the session.
    pub(super) fn idle(&self, session: &Session, stop: StopReason) -> Vec<Event> {
        vec![
            self.lifecycle(SessionEvent::ThreadStatusIdle {
                agent_name: self.agent.clone(),
                stop_reason: stop.clone(),
                stop_details: None,
            }),
            self.event(SessionEvent::Usage { usage: session.usage.clone(), budget: session.budget.clone() }),
            self.event(SessionEvent::StatusIdle { stop_reason: stop }),
        ]
    }
}

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

/// What the reply leads to.
pub(super) struct Led {
    pub(super) events: Vec<Event>,
    pub(super) end: StepEnd,
    /// The stop reason when the turn idles.
    pub(super) idle: Option<StopReason>,
}

/// The reply's events after `start`, and how the step ends. `cloud` keeps
/// the session running while its worker runs built-in calls; `more` means
/// the client sent events during the call, which the next call takes.
pub(super) fn led(
    turn: &Turn,
    r: &ModelResponse,
    start: &EventId,
    tools: &Toolbox,
    cloud: bool,
    more: bool,
) -> Led {
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
            Some(Disposition::Delegate) | None => {
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

/// The status a step leaves its session in.
pub(super) fn status(idle: Option<&StopReason>) -> SessionStatus {
    match idle {
        Some(_) => SessionStatus::Idle,
        None => SessionStatus::Running,
    }
}

/// The events one write lands, timed in order around the stamp of the
/// client events it reads, since the events list orders by `processed_at`.
pub(super) struct Batch {
    pub(super) read: Vec<EventId>,
    pub(super) at: Timestamp,
    pub(super) events: Vec<Event>,
}

/// `before`, then the stamp of `read`, then `after`, a microsecond apart.
pub(super) fn batch(read: Vec<EventId>, before: Vec<Event>, after: Vec<Event>) -> Batch {
    let start = Timestamp::now().as_datetime();
    let tick = |i: usize| {
        let micros = i64::try_from(i).unwrap_or(i64::MAX);
        Timestamp::from_datetime(start + chrono::Duration::microseconds(micros))
    };
    let n = before.len();
    let timed = |i: usize, e: Event| Event { processed_at: Some(tick(i)), ..e };
    let first = before.into_iter().enumerate().map(|(i, e)| timed(i, e));
    let then = after.into_iter().enumerate().map(|(j, e)| timed(n + 1 + j, e));
    Batch { read, at: tick(n), events: first.chain(then).collect() }
}
