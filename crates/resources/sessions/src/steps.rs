//! The step queue: one row per session that has work for the loop. A loop
//! worker claims a due row under a lease, runs one model call from the log
//! and commits its events, so a replica holds nothing between steps and a
//! step cut off by a crash runs again from the log after its lease.

use async_trait::async_trait;
use shared::WorkspaceId;

use crate::events::{Event, EventId};
use crate::store::StoreError;
use crate::types::{Session, SessionId};

/// A step a worker holds. A write that names it lands only while the worker
/// still holds the step with these counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The session's workspace.
    pub workspace: WorkspaceId,
    /// The session.
    pub session: SessionId,
    /// The worker that holds it.
    pub owner: String,
    /// The claims the step has had, this one included.
    pub attempts: i64,
    /// The wakes the step had when this claim took it.
    pub wakes: i64,
}

/// Changes a step makes to its session's row; `false` when nothing changed.
pub type SessionChange<'a> = dyn Fn(&mut Session) -> bool + Send + Sync + 'a;

/// What one write of a held step lands, in one transaction.
pub struct StepWrite<'a> {
    /// The client events the step read, stamped processed now.
    pub read: &'a [EventId],
    /// The events it appends, in order.
    pub events: &'a [Event],
    /// What it changes on the session's row.
    pub session: &'a SessionChange<'a>,
}

impl std::fmt::Debug for StepWrite<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepWrite")
            .field("read", &self.read)
            .field("events", &self.events)
            .finish_non_exhaustive()
    }
}

/// How a step ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEnd {
    /// Nothing is left to run until the session is woken again.
    Done,
    /// Another step runs at once (the turn goes on).
    Again,
    /// Done, and the session's stopped work item is queued again, for a
    /// worker to run the built-in calls the step parked.
    Requeue,
}

/// The rows the queue holds, as the gauges count them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepCounts {
    /// Due and held by no live lease.
    pub due: i64,
    /// Held under a lease that has not run out.
    pub leased: i64,
}

/// The step queue.
#[async_trait]
pub trait Steps: Send + Sync {
    /// The oldest due step, held by `owner` for `lease` seconds; `None` when
    /// none is due, or its workspace is at `per_workspace` held steps, which
    /// defers the workspace's waiting steps by a second.
    async fn claim(&self, owner: &str, lease: i64, per_workspace: i64) -> Result<Option<Claim>, StoreError>;
    /// Under the session's append lock, write `write` while `claim` still
    /// holds the step, and keep its lease for `lease` more seconds; `false`
    /// when it no longer does, with nothing written.
    async fn write(&self, claim: &Claim, lease: i64, write: StepWrite<'_>) -> Result<bool, StoreError>;
    /// Write the step's last events and release it as `end` says; `false`
    /// when another worker took it over, with nothing written.
    async fn commit(&self, claim: &Claim, end: StepEnd, write: StepWrite<'_>) -> Result<bool, StoreError>;
    /// Release the step unrun, due again in `seconds`.
    async fn defer(&self, claim: &Claim, seconds: i64) -> Result<(), StoreError>;
    /// The queue's rows, counted.
    async fn counts(&self) -> Result<StepCounts, StoreError>;
}

/// The workspaces whose sessions run on the loop, each with when it joined
/// (`AGENTD_LOOP_WORKSPACES`). A session created before its workspace joined
/// was started on the harness path; it moves to the loop at its next turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopWorkspaces(std::collections::BTreeMap<WorkspaceId, shared::Timestamp>);

impl LoopWorkspaces {
    /// From each workspace and when it joined.
    pub fn new(joined: impl IntoIterator<Item = (WorkspaceId, shared::Timestamp)>) -> Self {
        Self(joined.into_iter().collect())
    }

    /// When `ws` joined the loop, if it is listed.
    pub fn joined(&self, ws: &WorkspaceId) -> Option<shared::Timestamp> {
        self.0.get(ws).copied()
    }
}
