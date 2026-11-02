//! One thread's turn: rebuild its conversation from the log, then call the
//! model and run its tools until it stops, a call waits on the client, or the
//! session is interrupted. Every step is written as the session's events.
//!
//! ```text
//! thread_status_running → [ model_request_start → message → model_request_end
//!                           → tool_use… → tool_result… ]… → thread_status_idle
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sessions::events::{EventPayload, SessionEvent, StopReason};
use sessions::types::ThreadId;
use sessions::EngineContract;
use shared::ApiError;
use tokio::sync::watch;

use crate::model::{Block, Message, ModelClient, Role};
use crate::tools::{ToolExecutor, ToolOutcome};

mod act;
mod call;
pub mod history;
pub mod policy;
pub mod prompt;
mod resolve;
mod step;
mod thread;

pub use thread::{Delegation, Inbound, Inbox, ThreadCtx, TurnEnd};

use history::{rebuild, thread_events};

/// Most model calls one turn may make.
pub const MAX_STEPS: usize = 500;

/// Runs threads' turns for one session.
#[derive(Clone)]
pub struct Runner {
    /// The session's events and status.
    pub contract: Arc<dyn EngineContract>,
    /// The model.
    pub models: Arc<dyn ModelClient>,
    /// The built-in tools.
    pub tools: Arc<dyn ToolExecutor>,
    /// Delegation, for a coordinator's threads.
    pub delegation: Option<Arc<dyn Delegation>>,
    /// Rubric file texts, by file id.
    pub rubrics: Arc<HashMap<String, String>>,
    /// Becomes `true` when the session is interrupted.
    pub cancel: watch::Receiver<bool>,
    /// The output cap per model call.
    pub max_tokens: u32,
}

impl std::fmt::Debug for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner").finish_non_exhaustive()
    }
}

impl Runner {
    async fn emit(
        &self,
        t: &ThreadCtx,
        payload: impl Into<EventPayload> + Send,
    ) -> Result<sessions::Event, ApiError> {
        self.contract.emit(&t.ws, &t.session, t.scope.as_ref(), payload.into()).await
    }

    async fn lifecycle(&self, t: &ThreadCtx, payload: SessionEvent) -> Result<(), ApiError> {
        if !t.threaded {
            return Ok(());
        }
        self.contract.emit(&t.ws, &t.session, Some(&t.id), payload.into()).await.map(|_| ())
    }

    /// Run `t` until it is idle, blocked, interrupted or failed. A thread
    /// with nothing new to answer returns idle without running.
    pub async fn run(&self, t: &ThreadCtx, inbox: &Inbox) -> Result<TurnEnd, ApiError> {
        let queued = inbox.take();
        let mut all = self.contract.events(&t.ws, &t.session).await?;
        let mut h = rebuild(&thread_events(&all, t.scope.as_ref()), &self.rubrics);
        if !h.unanswered.is_empty() {
            let blocked = self.resolve(t, &all, &h.unanswered).await?;
            if !blocked.is_empty() {
                return Ok(TurnEnd::Blocked(blocked));
            }
            all = self.contract.events(&t.ws, &t.session).await?;
            h = rebuild(&thread_events(&all, t.scope.as_ref()), &self.rubrics);
        }
        let mut seen: HashSet<String> = all.iter().map(|e| e.id.as_str().to_owned()).collect();
        let mut messages = h.messages;
        append(&mut messages, queued, &mut seen);
        if messages.last().is_none_or(|m| m.role != Role::User) {
            return Ok(TurnEnd::Quiet);
        }
        if !already_running(&all, &t.id) {
            self.lifecycle(t, SessionEvent::ThreadStatusRunning { agent_name: t.name().to_owned() }).await?;
        }
        let end = self.turn(t, &mut messages, inbox, &mut seen).await?;
        if let (TurnEnd::Idle { reply }, Some(_), Some(d)) = (&end, &t.scope, &self.delegation) {
            d.reply(t, reply).await?;
        }
        let stop_reason = match &end {
            TurnEnd::Blocked(ids) => StopReason::RequiresAction { event_ids: ids.clone() },
            TurnEnd::Failed(_) => StopReason::RetriesExhausted {},
            TurnEnd::Quiet | TurnEnd::Idle { .. } | TurnEnd::Interrupted => StopReason::EndTurn {},
        };
        let idle = SessionEvent::ThreadStatusIdle {
            agent_name: t.name().to_owned(),
            stop_reason,
            stop_details: None,
        };
        self.lifecycle(t, idle).await?;
        Ok(end)
    }

    async fn turn(
        &self,
        t: &ThreadCtx,
        messages: &mut Vec<Message>,
        inbox: &Inbox,
        seen: &mut HashSet<String>,
    ) -> Result<TurnEnd, ApiError> {
        let mut reply = String::new();
        for _ in 0..MAX_STEPS {
            append(messages, inbox.take(), seen);
            if *self.cancel.borrow() {
                return Ok(TurnEnd::Interrupted);
            }
            match self.step(t, messages).await? {
                step::StepEnd::Continue { text } => reply = text.unwrap_or(reply),
                step::StepEnd::Done { text } => {
                    append(messages, inbox.take(), seen);
                    if messages.last().is_some_and(|m| m.role == Role::User) {
                        reply = text;
                        continue;
                    }
                    return Ok(TurnEnd::Idle { reply: text });
                }
                step::StepEnd::Stopped(end) => return Ok(end),
            }
        }
        Ok(TurnEnd::Idle { reply })
    }
}

/// Whether the log's last status for `thread` says it is running (a
/// delegation announces the thread before its message).
fn already_running(all: &[sessions::Event], thread: &ThreadId) -> bool {
    all.iter()
        .rev()
        .filter(|e| e.session_thread_id.as_ref() == Some(thread))
        .find_map(|e| match e.payload {
            EventPayload::Session(SessionEvent::ThreadStatusRunning { .. }) => Some(true),
            EventPayload::Session(
                SessionEvent::ThreadStatusIdle { .. } | SessionEvent::ThreadStatusTerminated { .. },
            ) => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

/// Add queued input the log did not already hold, as user text after the
/// last tool results.
fn append(messages: &mut Vec<Message>, queued: Vec<Inbound>, seen: &mut HashSet<String>) {
    for q in queued {
        if !seen.insert(q.event) {
            continue;
        }
        match messages.last_mut() {
            Some(m) if m.role == Role::User => m.content.push(Block::text(q.text)),
            _ => messages.push(Message { role: Role::User, content: vec![Block::text(q.text)] }),
        }
    }
}

/// A tool outcome as `agent.tool_result` content.
fn result_content(outcome: &ToolOutcome) -> Vec<shared::ContentBlock> {
    vec![shared::ContentBlock::text(outcome.output.clone())]
}
