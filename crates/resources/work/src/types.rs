//! Work wire types, as the SDK's `BetaSelfHostedWork` family defines them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shared::Timestamp;

impl WorkState {
    /// The state as stored and served.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Active => "active",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
        }
    }

    /// The state `raw` names.
    pub fn parse(raw: &str) -> Option<Self> {
        [Self::Queued, Self::Starting, Self::Active, Self::Stopping, Self::Stopped]
            .into_iter()
            .find(|s| s.as_str() == raw)
    }
}

/// A work item's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    /// Waiting for a worker.
    Queued,
    /// A worker acknowledged it.
    Starting,
    /// A worker holds its lease.
    Active,
    /// Asked to stop.
    Stopping,
    /// Done.
    Stopped,
}

/// Constant `"type":"session"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkDataTag {
    /// Always `session`.
    Session,
}

/// What the work is: a session to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkData {
    /// The session id.
    pub id: String,
    /// Always `session`.
    #[serde(rename = "type")]
    pub tag: WorkDataTag,
}

/// Constant `"type":"work"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkTag {
    /// Always `work`.
    Work,
}

/// A unit of work: one session's sandbox. As on live, its id is the
/// session's id, one item per session, reused for every run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// The session's id.
    pub id: String,
    /// When a worker acknowledged it.
    pub acknowledged_at: Option<Timestamp>,
    /// Always `null`, as live serves it.
    pub actor: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// The session.
    pub data: WorkData,
    /// The environment.
    pub environment_id: String,
    /// The latest heartbeat.
    pub latest_heartbeat_at: Option<Timestamp>,
    /// User metadata.
    pub metadata: BTreeMap<String, String>,
    /// The worker's credential payload: set only on the poll that hands the
    /// work out, `null` on every other read.
    pub secret: Option<String>,
    /// When it was queued, as live sets it.
    pub started_at: Option<Timestamp>,
    /// Its state.
    pub state: WorkState,
    /// When a stop was asked for.
    pub stop_requested_at: Option<Timestamp>,
    /// When it stopped.
    pub stopped_at: Option<Timestamp>,
    /// Constant tag.
    #[serde(rename = "type")]
    pub tag: WorkTag,
}

/// Constant `"type":"work_heartbeat"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatTag {
    /// Always `work_heartbeat`.
    WorkHeartbeat,
}

/// A heartbeat's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    /// The heartbeat just recorded; echo it as `expected_last_heartbeat` next.
    pub last_heartbeat: String,
    /// Whether the lease was extended.
    pub lease_extended: bool,
    /// The work's state.
    pub state: WorkState,
    /// The lease's time to live.
    pub ttl_seconds: i64,
    /// Constant tag.
    #[serde(rename = "type")]
    pub tag: HeartbeatTag,
}

/// Constant `"type":"work_queue_stats"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsTag {
    /// Always `work_queue_stats`.
    WorkQueueStats,
}

/// An environment's queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStats {
    /// Work waiting for a worker.
    pub depth: u64,
    /// The oldest work not yet acknowledged.
    pub oldest_queued_at: Option<Timestamp>,
    /// Work handed out but not acknowledged.
    pub pending: u64,
    /// Constant tag.
    #[serde(rename = "type")]
    pub tag: StatsTag,
    /// Workers that polled in the last 30 seconds.
    pub workers_polling: Option<u64>,
}
