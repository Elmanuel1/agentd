//! The session status events.

use serde::{Deserialize, Serialize};

use super::StopReason;

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
}
