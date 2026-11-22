//! Fitting a thread's prompt to its model's window before a call, and the
//! compaction call that runs instead of a turn once the prompt passes 80%
//! of it (CP-4): it writes the thread's checkpoint and keeps the session due,
//! so the next step takes the turn from the summary.

use std::collections::HashMap;

use agents::types::ModelSpeed;
use sessions::events::{AgentEvent, Event, EventId, SpanEvent, SpanModelUsage, StopReason};
use sessions::steps::{Claim, StepEnd};
use sessions::types::Session;
use shared::BackendError;

use super::Loop;
use super::grade::conversation;
use super::stall::abandoned;
use super::team::Thread;
use super::turn::{Turn, batch, thread_idle};
use super::usage::{kept, spent};
use crate::mcp::McpTool;
use crate::model::{Message, ModelError, ModelProvider};
use crate::turn::compact::checkpoint;
use crate::turn::compact::estimate::estimate;
use crate::turn::compact::prompt::cap;
use crate::turn::compact::{BYTES_PER_TOKEN, Tier, summary};
use crate::turn::history::{History, Kept, rebuild, thread_events};

/// A compaction the step runs instead of its turn.
pub(super) struct Compacting {
    messages: Vec<Message>,
    through: (EventId, i64),
    instructions: String,
    anthropic: bool,
}

/// The call a step opened: its turn, its thread and its `span.model_request_start`.
pub(super) struct Opened<'a> {
    pub(super) turn: &'a Turn,
    pub(super) thread: &'a Thread,
    pub(super) start: &'a EventId,
}

/// What the step's call needs from the thread's log.
pub(super) struct Log<'a> {
    pub(super) read: &'a [Event],
    pub(super) kept: &'a Kept,
    pub(super) rubrics: &'a HashMap<String, String>,
}

impl Loop {
    /// Cap `history`'s tool results; the compaction to run first when the prompt needs one (CP-4, CP-12) and
    /// the last call was not a compaction that left no checkpoint (CP-10).
    pub(super) async fn fit(
        &self,
        thread: &Thread,
        log: &Log<'_>,
        history: &mut History,
    ) -> Result<Option<Compacting>, ModelError> {
        let model = &thread.agent.config.model.id;
        let window = self.models.retry.window(self.models.client.as_ref(), model).await?;
        let scope = (!thread.primary).then(|| thread.id.clone());
        let events = thread_events(log.read, scope.as_ref());
        let view = checkpoint::view(&events, log.kept);
        let c = self.models.compaction;
        let tier = c.tier(estimate(&view), window);
        cap(&mut history.messages, c.tool_output_limit);
        let redo = view.checkpoint.as_ref().is_some_and(|cp| cp.model != *model);
        if (tier != Tier::Compact && !redo) || view.attempted {
            return Ok(None);
        }
        let Some(cut) = summary::cut(&view.prompt) else { return Ok(None) };
        // The oldest model calls go until the request fits the window (CP-11).
        let from = {
            let size = |from: usize| {
                let part = view.prompt.get(from..cut).unwrap_or_default();
                let n: usize = part.iter().map(|e| serde_json::to_string(e).map_or(0, |s| s.len())).sum();
                u64::try_from(n).unwrap_or(u64::MAX) / BYTES_PER_TOKEN
                    + view.checkpoint.as_ref().map_or(0, |c| c.bytes() / BYTES_PER_TOKEN)
            };
            let starts = view.prompt.iter().enumerate().filter(|(i, e)| {
                *i < cut
                    && matches!(
                        e.payload,
                        sessions::events::EventPayload::Span(SpanEvent::ModelRequestStart {})
                    )
            });
            let mut from = 0;
            for (i, _) in starts {
                if size(from) * 10 <= window * 9 {
                    break;
                }
                from = i;
            }
            from
        };
        let part: Vec<&Event> = view.prompt.get(from..cut).unwrap_or_default().to_vec();
        let mut summarised = rebuild(&part, log.rubrics, log.kept);
        if let Some(outcome) = &thread.grading {
            summarised = conversation(outcome, log.rubrics, summarised);
        }
        let mut messages = summarised.messages;
        if let Some(cp) = &view.checkpoint {
            messages.insert(0, cp.message());
        }
        cap(&mut messages, c.tool_output_limit);
        let Some(last) = cut.checked_sub(1).and_then(|i| view.prompt.get(i)) else { return Ok(None) };
        let seq = log.kept.get(&last.id).and_then(|k| k.get("seq")).and_then(serde_json::Value::as_i64);
        let through = (last.id.clone(), seq.unwrap_or_default());
        let anthropic = ModelProvider::of(model) == ModelProvider::Anthropic;
        let instructions = summary::instructions(&view.prompt, anthropic);
        Ok(Some(Compacting { messages, through, instructions, anthropic }))
    }

    /// Run `c` with the turn's system prompt and tools, so the prompt cache
    /// holds, and commit its call's end and the thread's checkpoint. A call
    /// that fails or leaves no summary is ended as it came; the next step
    /// takes the turn with the whole prompt.
    pub(super) async fn compact(
        &self,
        claim: &Claim,
        session: Session,
        opened: Opened<'_>,
        c: Compacting,
        mcp: &[McpTool],
    ) -> Result<(), BackendError> {
        let Opened { turn, thread, start } = opened;
        let (_, mut request) = self.request(&claim.workspace, &session, thread, c.messages, mcp).await;
        request.compact = Some(c.instructions);
        let model = request.model.clone();
        let provider = ModelProvider::of(&model).name();
        let (mut events, mut engine) = (Vec::new(), Vec::new());
        match self.models.retry.call(self.models.client.as_ref(), request).await {
            Ok(reply) => {
                let (usage, cost) = spent(&reply);
                if let Err(e) = self.models.budget.spend(provider, usage.input_tokens).await {
                    tracing::warn!(session = %claim.session, error = %e, "loop: could not count the call's tokens");
                }
                let end = turn.event(SpanEvent::ModelRequestEnd {
                    model_request_start_id: start.clone(),
                    is_error: false,
                    model_usage: SpanModelUsage {
                        tokens: reply.usage.clone(),
                        speed: Some(ModelSpeed::Standard),
                    },
                });
                engine.extend(cost.map(|c| (end.id.clone(), kept(c))));
                events.push(end);
                let cp = summary::checkpoint(&reply, &model, c.anthropic, c.through);
                crate::metrics::compaction(cp.as_ref().map_or("no_summary", |_| "compacted"));
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
        self.commit(claim, StepEnd::Again, &done, None).await?;
        Ok(())
    }

    /// A call that cannot start because its model has no window ends the
    /// turn as a failed model call; a grader's round fails with it.
    pub(super) async fn unfit(
        &self,
        claim: &Claim,
        opened: Opened<'_>,
        session: &Session,
        e: &ModelError,
    ) -> Result<(), BackendError> {
        let Opened { turn, thread, start } = opened;
        let mut events = turn.failed(start, Some(e));
        events.push(turn.lifecycle(thread_idle(turn, StopReason::RetriesExhausted {})));
        let why = "The grader's model could not be called, before it gave a grade.";
        let failed = abandoned(session, thread, why);
        events.extend(failed.as_ref().map(|(end, _)| end.clone()));
        let verdict = failed.map(|(_, eval)| eval);
        self.commit(claim, StepEnd::Done, &batch(Vec::new(), Vec::new(), events), verdict.as_ref()).await?;
        Ok(())
    }
}
