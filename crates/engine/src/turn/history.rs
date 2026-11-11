//! The conversation a thread's model sees, rebuilt from the event log. The
//! log is the only state a turn needs, so any replica can pick a session up.
//!
//! Each `span.model_request_start` opens a step: what the agent emitted goes
//! to its assistant message, and every user-side input up to the next step
//! (tool results first, then text) to the user message after it.

use std::collections::{HashMap, HashSet};

use sessions::events::{AgentEvent, Event, EventPayload, Rubric, SpanEvent, UserEvent};
use sessions::types::{OutcomeResult, ThreadId};
use shared::ContentBlock;

pub use super::prompt::{feedback_prompt, outcome_prompt};
use crate::model::{Block, Message, Role};
use builder::Builder;

/// The coordinator's delegation tool, as history replays it.
pub const SEND_TO_AGENT: &str = "send_to_agent";

/// A tool call the client still has to answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Unanswered {
    /// The `agent.tool_use` / `agent.custom_tool_use` event id.
    pub id: String,
    /// The tool.
    pub name: String,
    /// Its input.
    pub input: serde_json::Value,
    /// A custom tool (the client runs it) rather than a confirmation.
    pub custom: bool,
}

/// What a thread's log says.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    /// The conversation.
    pub messages: Vec<Message>,
    /// Tool calls awaiting the client, oldest first.
    pub unanswered: Vec<Unanswered>,
}

impl History {
    /// Whether the conversation ends on input the model has not answered.
    pub fn awaits_model(&self) -> bool {
        self.unanswered.is_empty() && self.messages.last().is_some_and(|m| m.role == Role::User)
    }
}

/// The events of one thread: `None` is the primary thread, which owns the
/// client events but for answers to another thread's calls; a secondary
/// thread gets the client answers to its own tool calls.
pub fn thread_events<'a>(all: &'a [Event], scope: Option<&ThreadId>) -> Vec<&'a Event> {
    let own = |e: &Event| e.session_thread_id.as_ref() == scope;
    let calls: HashSet<&str> = all.iter().filter(|e| own(e)).map(|e| e.id.as_str()).collect();
    all.iter()
        .filter(|e| match (&e.payload, scope) {
            (EventPayload::User(u), Some(_)) => answer_to(u).is_some_and(|id| calls.contains(id)),
            (EventPayload::User(u), None) => answer_to(u).is_none_or(|id| calls.contains(id)),
            (EventPayload::Session(_), _) => false,
            _ => own(e),
        })
        .collect()
}

fn answer_to(u: &UserEvent) -> Option<&str> {
    match u {
        UserEvent::CustomToolResult { custom_tool_use_id, .. } => Some(custom_tool_use_id.as_str()),
        UserEvent::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
        _ => None,
    }
}

fn texts(content: &[ContentBlock]) -> String {
    content.iter().filter_map(ContentBlock::as_text).collect::<Vec<_>>().join("\n")
}

/// Rebuild `events` (one thread's, oldest first) into its conversation.
/// `rubrics` holds the text of rubric files by file id. A tool call with no
/// result that the client does not owe gets an error result, so the
/// conversation stays valid after an interrupted step.
pub fn rebuild(events: &[&Event], rubrics: &HashMap<String, String>) -> History {
    let mut b = Builder::default();
    for ev in events {
        let id = ev.id.as_str().to_owned();
        match &ev.payload {
            EventPayload::Span(SpanEvent::ModelRequestStart {}) => b.step(),
            EventPayload::Span(SpanEvent::OutcomeEvaluationEnd { result, explanation, .. })
                if *result != OutcomeResult::Satisfied =>
            {
                b.text(feedback_prompt(*result, explanation.as_deref().unwrap_or_default()));
            }
            EventPayload::Agent(AgentEvent::Message { content }) => {
                b.assistant.push(Block::text(texts(content)))
            }
            EventPayload::Agent(AgentEvent::ToolUse { name, input, evaluated_permission, .. }) => {
                b.call(id.clone(), name, input);
                if evaluated_permission.as_deref() == Some("ask") {
                    b.owed.insert(id);
                }
            }
            EventPayload::Agent(AgentEvent::CustomToolUse { name, input }) => {
                b.call(id.clone(), name, input);
                b.custom.insert(id);
            }
            EventPayload::Agent(AgentEvent::McpToolUse { name, input, .. }) => b.call(id, name, input),
            EventPayload::Agent(AgentEvent::ThreadMessageSent {
                content,
                to_agent_name,
                to_session_thread_id,
            }) => {
                if ev.session_thread_id.is_none() {
                    let input = serde_json::json!({"agent_name": to_agent_name, "session_thread_id": to_session_thread_id, "message": texts(content)});
                    b.call(id.clone(), SEND_TO_AGENT, &input);
                    let note = format!("Sent to thread {to_session_thread_id}. The reply arrives as a message from that thread.");
                    b.result(id, note, false);
                }
            }
            EventPayload::Agent(AgentEvent::ToolResult { tool_use_id, content, is_error })
            | EventPayload::Agent(AgentEvent::McpToolResult {
                mcp_tool_use_id: tool_use_id,
                content,
                is_error,
            }) => {
                b.result(tool_use_id.as_str().to_owned(), texts(content), *is_error);
            }
            EventPayload::Agent(AgentEvent::ThreadMessageReceived {
                content,
                from_agent_name,
                from_session_thread_id,
            }) => {
                if ev.session_thread_id.is_none() {
                    b.text(format!(
                        "Message from {from_agent_name} (thread {from_session_thread_id}):\n{}",
                        texts(content)
                    ));
                } else {
                    b.text(texts(content));
                }
            }
            EventPayload::User(u) if ev.processed_at.is_some() => match u {
                UserEvent::Message { content } => b.text(texts(content)),
                UserEvent::SystemMessage { content } => {
                    b.text(format!("<system>{}</system>", texts(content)))
                }
                UserEvent::DefineOutcome { description, rubric, .. } => {
                    let rubric = match rubric {
                        Rubric::Text { content } => content.clone(),
                        Rubric::File { file_id } => rubrics.get(file_id).cloned().unwrap_or_default(),
                    };
                    b.text(outcome_prompt(description, &rubric));
                }
                UserEvent::CustomToolResult { custom_tool_use_id: tid, content, is_error }
                | UserEvent::ToolResult { tool_use_id: tid, content, is_error } => {
                    b.result(tid.as_str().to_owned(), texts(content), *is_error);
                }
                UserEvent::Interrupt {} | UserEvent::ToolConfirmation { .. } => {}
            },
            _ => {}
        }
    }
    b.finish()
}

mod builder;

#[cfg(test)]
mod tests;
