//! Threads (L-15): which thread a step runs, and delegation. A coordinator's
//! `create_agent` opens a roster agent's thread and `send_to_agent` follows
//! one up; the thread runs in later steps on its own, and its final text goes
//! back to the coordinator as a message from that thread.

use agents::AgentSnapshot;
use serde_json::Value;
use sessions::events::{AgentEvent, Event, EventPayload, SessionEvent};
use sessions::types::{Session, ThreadId, primary_thread_id};
use shared::ContentBlock;

use super::grade::Outcome;
use super::status::{busy, parked};
use super::turn::Turn;
use crate::turn::delegation::{CREATE_AGENT, LIST_AGENTS, SEND_TO_AGENT, listing};

/// A thread a step can run, with the agent it runs.
#[derive(Clone)]
pub(super) struct Thread {
    pub(super) id: ThreadId,
    pub(super) agent: AgentSnapshot,
    pub(super) primary: bool,
    /// The outcome round this thread grades, for the grader's thread.
    pub(super) grading: Option<Outcome>,
}

impl Thread {
    /// The session's primary thread, which runs the session's agent.
    pub(super) fn primary(session: &Session) -> Self {
        Self {
            id: primary_thread_id(&session.id),
            agent: session.agent.clone(),
            primary: true,
            grading: None,
        }
    }

    pub(super) fn turn(&self) -> Turn {
        Turn { thread: self.id.clone(), agent: self.agent.config.name.clone(), scoped: !self.primary }
    }
}

pub(super) fn roster(session: &Session) -> Vec<&AgentSnapshot> {
    session.agent.config.multiagent.as_ref().map(|m| m.roster_agents()).unwrap_or_default()
}

/// The primary thread, then every thread the log opened, oldest first.
pub(super) fn threads(session: &Session, log: &[Event]) -> Vec<Thread> {
    let primary = Thread::primary(session);
    let roster = roster(session);
    let opened = log.iter().filter_map(|e| match (&e.payload, &e.session_thread_id) {
        (EventPayload::Session(SessionEvent::ThreadCreated { agent_name }), Some(id)) => {
            let agent = roster.iter().find(|a| a.config.name == *agent_name)?;
            Some(Thread { id: id.clone(), agent: (*agent).clone(), primary: false, grading: None })
        }
        _ => None,
    });
    std::iter::once(primary).chain(opened).collect()
}

/// A coordinator's call to a roster tool: its `agent.tool_use` and what it
/// leads to, in live's order.
pub(super) fn delegate(
    turn: &Turn,
    session: &Session,
    log: &[Event],
    name: &str,
    input: &Value,
) -> Vec<Event> {
    let call = turn.event(AgentEvent::ToolUse {
        name: name.to_owned(),
        input: input.clone(),
        evaluated_permission: Some("allow".into()),
        evaluation: None,
    });
    let answer = |text: String, is_error: bool| {
        turn.event(AgentEvent::ToolResult {
            tool_use_id: call.id.clone(),
            content: vec![ContentBlock::text(text)],
            is_error,
        })
    };
    if name == LIST_AGENTS {
        let started: Vec<(ThreadId, String, bool, Option<&str>)> = threads(session, log)
            .into_iter()
            .filter(|t| !t.primary)
            .map(|t| (t.id.clone(), t.agent.config.name.clone(), busy(log, &t.id), parked(log, &t.id)))
            .collect();
        let result = answer(listing(&started, &roster(session)), false);
        return vec![call, result];
    }
    match send(turn, session, log, name, input) {
        Ok(events) => events,
        Err(why) => {
            let result = answer(why, true);
            vec![call, result]
        }
    }
}

fn send(
    turn: &Turn,
    session: &Session,
    log: &[Event],
    name: &str,
    input: &Value,
) -> Result<Vec<Event>, String> {
    let field =
        |key: &str| input.get(key).and_then(Value::as_str).ok_or(format!("{name}: `{key}` is required"));
    let (thread, opened, text) = if name == CREATE_AGENT {
        let (agent_name, task) = (field("agent_name")?, field("task")?);
        let roster = roster(session);
        let agent = roster.iter().find(|a| a.config.name == agent_name).ok_or_else(|| {
            let names: Vec<&str> = roster.iter().map(|a| a.config.name.as_str()).collect();
            format!("`{agent_name}` is not on your roster ({})", names.join(", "))
        })?;
        (
            Thread { id: ThreadId::generate(), agent: (*agent).clone(), primary: false, grading: None },
            true,
            task,
        )
    } else {
        let (tid, message) = (field("thread_id")?, field("message")?);
        let thread = threads(session, log).into_iter().find(|t| !t.primary && t.id.as_str() == tid);
        let thread = thread.ok_or_else(|| {
            format!("{SEND_TO_AGENT}: no thread {tid} of yours; {LIST_AGENTS} gives your threads' ids")
        })?;
        (thread, false, message)
    };
    let to = thread.turn();
    let content = vec![ContentBlock::text(text)];
    let mut events = Vec::new();
    if opened {
        events.push(to.lifecycle(SessionEvent::ThreadCreated { agent_name: to.agent.clone() }));
    }
    events.push(turn.event(AgentEvent::ThreadMessageSent {
        content: content.clone(),
        to_agent_name: Some(to.agent.clone()),
        to_session_thread_id: to.thread.clone(),
    }));
    // Live: a thread is running before the message it runs on arrives.
    if !busy(log, &to.thread) {
        events.push(to.lifecycle(SessionEvent::ThreadStatusRunning { agent_name: to.agent.clone() }));
    }
    events.push(to.event(AgentEvent::ThreadMessageReceived {
        content,
        from_agent_name: turn.agent.clone(),
        from_session_thread_id: turn.thread.clone(),
    }));
    Ok(events)
}

/// A delegate's final text, sent back to its coordinator.
pub(super) fn reply(from: &Turn, primary: &Turn, text: &str) -> Vec<Event> {
    let content = vec![ContentBlock::text(text)];
    vec![
        from.event(AgentEvent::ThreadMessageSent {
            content: content.clone(),
            to_agent_name: None,
            to_session_thread_id: primary.thread.clone(),
        }),
        primary.event(AgentEvent::ThreadMessageReceived {
            content,
            from_agent_name: from.agent.clone(),
            from_session_thread_id: from.thread.clone(),
        }),
    ]
}
