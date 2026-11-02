//! Session threads: the primary thread runs the session's agent; each
//! roster agent a coordinator delegates to runs in a thread of its own.
//! Threads are read from the event log, which is their only record.

use agents::AgentSnapshot;
use serde::{Deserialize, Serialize};
use shared::{Id, Resource, Timestamp, Usage};

use super::{SessionId, Stats};

/// Marker for `sthr_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct ThreadResource_;
impl Resource for ThreadResource_ {
    const PREFIX: &'static str = "sthr";
    const LABEL: &'static str = "session thread";
}
/// `Id<ThreadResource_>` alias.
pub type ThreadId = Id<ThreadResource_>;

/// The primary thread's id: the session id's body under the thread prefix,
/// so it is known without being stored.
pub fn primary_thread_id(session: &SessionId) -> ThreadId {
    let body = session.as_str().strip_prefix("sesn_").unwrap_or(session.as_str());
    ThreadId::parse(&format!("sthr_{body}")).unwrap_or_else(|_| ThreadId::generate())
}

/// A thread's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    /// Waiting for input.
    Idle,
    /// Working.
    Running,
    /// Finished for good.
    Terminated,
}

/// Constant `"type":"session_thread"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadTag {
    /// Always `session_thread`.
    SessionThread,
}

/// Thread timing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadStats {
    /// Seconds spent working.
    pub active_seconds: u64,
    /// Seconds since the thread was created.
    pub duration_seconds: u64,
    /// Seconds before the thread first ran.
    pub startup_seconds: u64,
}

impl From<Stats> for ThreadStats {
    fn from(s: Stats) -> Self {
        Self { active_seconds: s.active_seconds, duration_seconds: s.duration_seconds, startup_seconds: 0 }
    }
}

/// The wire session thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionThread {
    /// The agent the thread runs.
    pub agent: AgentSnapshot,
    /// Archive stamp.
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// `sthr_…`.
    pub id: ThreadId,
    /// The thread that started it; `null` for the primary thread.
    pub parent_thread_id: Option<ThreadId>,
    /// The session.
    pub session_id: SessionId,
    /// Timing.
    pub stats: ThreadStats,
    /// Status.
    pub status: ThreadStatus,
    /// Constant tag.
    #[serde(rename = "type")]
    pub tag: ThreadTag,
    /// Last change.
    pub updated_at: Timestamp,
    /// Token usage of the thread's model calls.
    pub usage: Usage,
}
