//! A thread as the loop runs it: its context, how its turn ends, its queue of
//! new input, and the delegation a coordinator's thread may use.

use agents::AgentSnapshot;
use async_trait::async_trait;
use parking_lot::Mutex;
use sessions::events::EventId;
use sessions::types::{SessionId, ThreadId};
use shared::{ApiError, WorkspaceId};

use super::policy::Toolbox;

/// One thread of a session, ready to run.
#[derive(Debug, Clone)]
pub struct ThreadCtx {
    /// The workspace.
    pub ws: WorkspaceId,
    /// The session.
    pub session: SessionId,
    /// The thread's id.
    pub id: ThreadId,
    /// How its events are tagged: `None` for the primary thread.
    pub scope: Option<ThreadId>,
    /// The agent it runs.
    pub agent: AgentSnapshot,
    /// What the agent's model may call.
    pub toolbox: Toolbox,
    /// The system prompt.
    pub system: String,
    /// The session has threads (a coordinator's); only then do thread
    /// lifecycle events go out, as a single-agent session has none.
    pub threaded: bool,
}

impl ThreadCtx {
    /// The agent's name.
    pub fn name(&self) -> &str {
        &self.agent.config.name
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnEnd {
    /// Nothing new to answer; the thread did not run.
    Quiet,
    /// The model finished; `reply` is its last text.
    Idle {
        /// What the model last said.
        reply: String,
    },
    /// Calls wait on the client: these events.
    Blocked(Vec<EventId>),
    /// The session was interrupted.
    Interrupted,
    /// The model could not be reached; the thread stops.
    Failed(String),
}

/// Input that reached a thread while it may be running: the event that
/// records it and the text the model gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbound {
    /// The event id in the log.
    pub event: String,
    /// The text.
    pub text: String,
}

/// A thread's queue of new input.
#[derive(Debug, Default)]
pub struct Inbox(Mutex<Vec<Inbound>>);

impl Inbox {
    /// Queue `item`.
    pub fn push(&self, item: Inbound) {
        self.0.lock().push(item);
    }

    /// Take everything queued.
    pub fn take(&self) -> Vec<Inbound> {
        std::mem::take(&mut *self.0.lock())
    }

    /// Whether anything is queued.
    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }
}

/// The coordinator's delegation, provided by the session worker.
#[async_trait]
pub trait Delegation: Send + Sync {
    /// Deliver a `send_to_agent` call; the result text for the model, or the
    /// reason it could not be sent. Writes the delegation events itself and
    /// returns the `agent.thread_message_sent` event.
    async fn send(&self, from: &ThreadCtx, input: &serde_json::Value) -> Result<(EventId, String), String>;
    /// The roster, for `list_agents`.
    fn roster(&self) -> String;
    /// Hand a delegated thread's final text back to the coordinator.
    async fn reply(&self, from: &ThreadCtx, text: &str) -> Result<(), ApiError>;
}
