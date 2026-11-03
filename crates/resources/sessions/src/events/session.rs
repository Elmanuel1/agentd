//! The session status events.

use serde::{Deserialize, Serialize};

use shared::Usage;

use super::StopReason;
use crate::types::Budget;

/// The session status events (`type` `session.*`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEvent {
    /// Awaiting input; inspect `stop_reason`.
    #[serde(rename = "session.status_idle")]
    StatusIdle {
        /// Why.
        stop_reason: StopReason,
    },
    /// Actively working.
    #[serde(rename = "session.status_running")]
    StatusRunning {},
    /// Rescheduling after a retryable error.
    #[serde(rename = "session.status_rescheduled")]
    StatusRescheduled {},
    /// Terminal.
    #[serde(rename = "session.status_terminated")]
    StatusTerminated {},
    /// An error surfaced mid-run.
    #[serde(rename = "session.error")]
    Error {
        /// Error detail.
        error: serde_json::Value,
    },
    /// The session's cumulative usage and list cost, with its budget, at the
    /// time of the snapshot.
    #[serde(rename = "session.usage")]
    Usage {
        /// Cumulative usage.
        usage: Usage,
        /// The budget, `null` when the session has none.
        #[serde(default)]
        budget: Option<Budget>,
    },
    /// The session was deleted; the last event on its stream.
    #[serde(rename = "session.deleted")]
    Deleted {},
    /// A coordinator started a thread for a roster agent. The thread's id is
    /// the envelope's `session_thread_id`.
    #[serde(rename = "session.thread_created")]
    ThreadCreated {
        /// The agent the thread runs.
        agent_name: String,
    },
    /// A thread started working.
    #[serde(rename = "session.thread_status_running")]
    ThreadStatusRunning {
        /// The agent the thread runs.
        agent_name: String,
    },
    /// A thread is waiting for input.
    #[serde(rename = "session.thread_status_idle")]
    ThreadStatusIdle {
        /// The agent the thread runs.
        agent_name: String,
        /// Why it stopped.
        stop_reason: StopReason,
        /// Live sends `null`.
        stop_details: Option<serde_json::Value>,
    },
    /// A thread finished for good.
    #[serde(rename = "session.thread_status_terminated")]
    ThreadStatusTerminated {
        /// The agent the thread runs.
        agent_name: String,
    },
}
