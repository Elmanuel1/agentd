//! Fitting a thread's prompt to its model's window before a call, and the
//! compaction call that runs instead of a turn once the prompt passes 80%
//! of it (CP-4): it writes the thread's checkpoint and keeps the session due,
//! so the next step takes the turn from the summary.

use std::collections::HashMap;

use agents::types::ModelSpeed;
use sessions::events::{AgentEvent, Event, EventId, SpanEvent, SpanModelUsage, StopReason};
use sessions::steps::{Claim, StepEnd};
use sessions::types::{Session, SessionStatus};
use shared::BackendError;

use super::grade::conversation;
use super::team::Thread;
use super::turn::{batch, Turn};
use super::usage::{charged, spent};
use super::Loop;
use crate::mcp::McpTool;
use crate::model::{Message, ModelError, ModelProvider};
use crate::turn::compact::checkpoint::{self, View};
use crate::turn::compact::estimate::estimate;
use crate::turn::compact::prompt::{cap, clear};
use crate::turn::compact::{summary, Tier, BYTES_PER_TOKEN};
use crate::turn::history::{rebuild, thread_events, History, Kept};

/// A compaction the step runs instead of its turn.
pub(super) struct Compacting {
    messages: Vec<Message>,
    through: EventId,
    instructions: String,
    anthropic: bool,
}

/// What the step's call needs from the thread's log.
pub(super) struct Log<'a> {
    pub(super) read: &'a [Event],
    pub(super) kept: &'a Kept,
    pub(super) rubrics: &'a HashMap<String, String>,
}

impl Loop {
    /// Cap and, past the clear tier, clear `history`'s tool results; the
    /// compaction to run first when the prompt needs one (CP-4, CP-12) and
    /// the last call was not a compaction that left no checkpoint (CP-10).
    pub(super) async fn fit(
        &self,
        thread: &Thread,
        log: &Log<'_>,
        history: &mut History,
    ) -> Result<Option<Compacting>, ModelError> {
        let model = &thread.agent.config.model.id;
        let window = self.models.client.window(model).await?;
        let scope = (!thread.primary).then(|| thread.id.clone());
        let events = thread_events(log.read, scope.as_ref());
        let view = checkpoint::view(&events, log.kept);
        let c = self.models.compaction;
        let tier = c.tier(estimate(&view), window);
        let tidy = |messages: &mut Vec<Message>| {
            cap(messages, c.tool_output_limit);
            if tier != Tier::Whole {
                clear(messages, c.keep_results);
            }
        };
        tidy(&mut history.messages);
        let redo = view.checkpoint.as_ref().is_some_and(|cp| cp.model != *model);
        if (tier != Tier::Compact && !redo) || view.attempted {
            return Ok(None);
        }
        let Some(cut) = summary::cut(&view.prompt) else { return Ok(None) };
        let from = fitting(&view, cut, window);
        let part: Vec<&Event> = view.prompt.get(from..cut).unwrap_or_default().to_vec();
        let mut summarised = rebuild(&part, log.rubrics, log.kept);
        if let Some(outcome) = &thread.grading {
            summarised = conversation(outcome, log.rubrics, summarised);
        }
        let mut messages = summarised.messages;
        if let Some(cp) = &view.checkpoint {
            messages.insert(0, cp.message());
        }
        tidy(&mut messages);
        let Some(through) = cut.checked_sub(1).and_then(|i| view.prompt.get(i)).map(|e| e.id.clone()) else {
            return Ok(None);
        };
        let anthropic = ModelProvider::of(model) == ModelProvider::Anthropic;
        let instructions = summary::instructions(&view.prompt, anthropic);
        Ok(Some(Compacting { messages, through, instructions, anthropic }))
    }

    /// Run `c` with the turn's system prompt and tools, so the prompt cache
    /// holds, and commit its call's end and the thread's checkpoint. A call
    /// that fails or leaves no summary is ended as it came; the next step
    /// takes the turn with the cleared prompt.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn compact(
        &self,
        claim: &Claim,
        turn: &Turn,
        thread: &Thread,
        mut session: Session,
        start: &EventId,
        c: Compacting,
        mcp: &[McpTool],
    ) -> Result<(), BackendError> {
        let (_, mut request) = self.request(&session, thread, c.messages, mcp);
        request.compact = Some(c.instructions);
        let model = request.model.clone();
        let provider = ModelProvider::of(&model).name();
        let (mut events, mut engine) = (Vec::new(), Vec::new());
        match self.models.retry.call(self.models.client.as_ref(), request).await {
            Ok(reply) => {
                let (usage, cents) = spent(&reply);
                if let Err(e) = self.models.budget.spend(provider, usage.input_tokens).await {
                    tracing::warn!(session = %claim.session, error = %e, "loop: could not count the call's tokens");
                }
                charged(&mut session, &usage, cents);
                events.push(turn.event(SpanEvent::ModelRequestEnd {
                    model_request_start_id: start.clone(),
                    is_error: false,
                    model_usage: SpanModelUsage {
                        tokens: reply.usage.clone(),
                        speed: Some(ModelSpeed::Standard),
                    },
                }));
                let cp = summary::checkpoint(&reply, &model, c.anthropic, c.through);
                crate::metrics::compaction(if cp.is_some() { "compacted" } else { "no_summary" });
                if let Some(cp) = cp {
                    let compacted = turn.event(AgentEvent::ThreadContextCompacted {});
                    engine.push((compacted.id.clone(), cp.kept()));
                    events.push(compacted);
                }
            }
            Err(e) => {
                tracing::warn!(session = %claim.session, error = %e, "loop: a compaction call failed");
                crate::metrics::compaction("failed");
                events.extend(turn.failed(start, None));
            }
        }
        let mut done = batch(Vec::new(), Vec::new(), events);
        done.engine = engine;
        self.commit(claim, StepEnd::Again, &done, &session).await?;
        Ok(())
    }

    /// A call that cannot start because its model has no window ends the
    /// turn as a failed model call.
    pub(super) async fn unfit(
        &self,
        claim: &Claim,
        turn: &Turn,
        mut session: Session,
        start: &EventId,
        e: &ModelError,
    ) -> Result<(), BackendError> {
        session.status = SessionStatus::Idle;
        let mut events = turn.failed(start, Some(e));
        events.extend(turn.idle(&session, StopReason::RetriesExhausted {}));
        self.commit(claim, StepEnd::Done, &batch(Vec::new(), Vec::new(), events), &session).await?;
        Ok(())
    }
}

/// The first event of `view`'s prompt a compaction request up to `cut`
/// keeps: the oldest model calls go until it fits the window (CP-11).
fn fitting(view: &View<'_>, cut: usize, window: u64) -> usize {
    let size = |from: usize| {
        let part = view.prompt.get(from..cut).unwrap_or_default();
        let n: usize = part.iter().map(|e| serde_json::to_string(e).map_or(0, |s| s.len())).sum();
        u64::try_from(n).unwrap_or(u64::MAX) / BYTES_PER_TOKEN
            + view.checkpoint.as_ref().map_or(0, |c| c.bytes() / BYTES_PER_TOKEN)
    };
    let starts = view.prompt.iter().enumerate().filter(|(i, e)| {
        *i < cut && matches!(e.payload, sessions::events::EventPayload::Span(SpanEvent::ModelRequestStart {}))
    });
    let mut from = 0;
    for (i, _) in starts {
        if size(from) * 10 <= window * 9 {
            break;
        }
        from = i;
    }
    from
}
