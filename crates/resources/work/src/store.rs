//! Work item persistence. One item per session, keyed by its workspace and
//! session; a queue is a self-hosted environment's id, the runtime queue, or
//! the hosted queue of sessions whose own sandbox takes their work.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use shared::{OrgId, Pagination, Timestamp, WorkspaceId};

use crate::types::{Work, WorkState};

/// The runtime queue: `cloud` sessions' work, polled with agentd's runtime key.
pub const RUNTIME: &str = "runtime";
/// Sessions whose own sandbox's harness takes their work; no poll hands it out.
pub const HOSTED: &str = "hosted";

/// Store-level errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    /// The backing store failed.
    #[error("work store backend: {0}")]
    Backend(#[source] shared::BackendError),
}

/// A work item with what agentd keeps beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItem {
    /// Its workspace.
    pub workspace: WorkspaceId,
    /// Its organization.
    pub org: OrgId,
    /// The queue it waits on.
    pub queue: String,
    /// The wire object.
    pub work: Work,
    /// The latest heartbeat as handed to the worker.
    pub last_heartbeat: Option<String>,
    /// When the lease runs out.
    pub lease_until: Option<Timestamp>,
    /// When a poll last handed it out.
    pub handed_out_at: Option<Timestamp>,
}

impl WorkItem {
    /// The item as served at `now`: a `stopping` item whose lease ran out is
    /// `stopped`, from when it ran out.
    #[must_use]
    pub fn served(mut self, now: Timestamp) -> Self {
        let lapsed = self.lease_until.filter(|l| l.as_datetime() < now.as_datetime());
        if let (WorkState::Stopping, Some(at)) = (self.work.state, lapsed) {
            self.work.state = WorkState::Stopped;
            self.work.stopped_at = Some(at);
        }
        self
    }
}

/// Which items a request's path reaches: those of an environment, those on a
/// queue, or both. A write misses an item it does not reach; the default,
/// with neither, reaches any item, for agentd's own calls.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reach {
    /// Items whose `environment_id` is this.
    pub environment: Option<String>,
    /// Items on this queue.
    pub queue: Option<String>,
}

/// The items a list or stats call covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// One workspace's items of one environment, on one queue when set.
    Environment {
        /// The workspace.
        workspace: WorkspaceId,
        /// The environment.
        environment: String,
        /// Only this queue's items.
        queue: Option<String>,
    },
    /// The items on one queue.
    Queue(String),
}

/// What a heartbeat sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beat {
    /// The heartbeat the worker last saw, when it names one.
    pub expected: Option<String>,
    /// This heartbeat, as handed back.
    pub stamp: String,
    /// When it lands.
    pub at: Timestamp,
    /// The lease it grants.
    pub lease_until: Timestamp,
}

/// A queue's counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Queued and not handed out.
    pub depth: u64,
    /// Handed out and not acknowledged.
    pub pending: u64,
    /// The oldest queued item.
    pub oldest_queued_at: Option<Timestamp>,
}

/// Reads of work items.
#[async_trait]
pub trait WorkReads: Send + Sync {
    /// `session`'s item.
    async fn get(&self, ws: &WorkspaceId, session: &str) -> Result<Option<WorkItem>, StoreError>;
    /// `session`'s item on `queue`, in any workspace.
    async fn find_on_queue(&self, queue: &str, session: &str) -> Result<Option<WorkItem>, StoreError>;
    /// One page of `scope`'s items, newest first.
    async fn list(&self, scope: &Scope, page: &Pagination) -> Result<Vec<WorkItem>, StoreError>;
    /// `scope`'s queued counts.
    async fn counts(&self, scope: &Scope, reclaim: Duration, now: Timestamp) -> Result<Counts, StoreError>;
    /// The organization and workspace of an item on the runtime queue in
    /// `environment`, if there is one.
    async fn runtime_owner(&self, environment: &str) -> Result<Option<(OrgId, WorkspaceId)>, StoreError>;
}

/// Persistence port for work items: its writes, and its reads.
#[async_trait]
pub trait WorkStore: WorkReads {
    /// Insert `item`.
    async fn insert(&self, item: &WorkItem) -> Result<(), StoreError>;
    /// Hand out the oldest queued item of `queue` not handed out within
    /// `reclaim`, marking it handed out at `now`.
    async fn claim(
        &self,
        queue: &str,
        reclaim: Duration,
        now: Timestamp,
    ) -> Result<Option<WorkItem>, StoreError>;
    /// Merge `set` into the item's metadata and drop `remove`.
    async fn update_metadata(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        set: &BTreeMap<String, String>,
        remove: &[String],
    ) -> Result<Option<WorkItem>, StoreError>;
    /// A worker took a queued item: `starting`, leased until `lease_until`.
    /// Any other state is left as it is.
    async fn ack(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        now: Timestamp,
        lease_until: Timestamp,
    ) -> Result<Option<WorkItem>, StoreError>;
    /// Extend the lease of an item not stopping or stopped whose last
    /// heartbeat is `beat.expected`; `true` with the item when it did.
    async fn heartbeat(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        beat: &Beat,
    ) -> Result<Option<(WorkItem, bool)>, StoreError>;
    /// Ask the item to stop: `stopped` when `force` or no worker holds it,
    /// else `stopping`.
    async fn stop(
        &self,
        ws: &WorkspaceId,
        session: &str,
        reach: &Reach,
        force: bool,
        now: Timestamp,
    ) -> Result<Option<WorkItem>, StoreError>;
}
