//! The work items, in this process. Like the event stream's broadcast, the
//! board is per replica.

use std::collections::HashMap;

use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::types::{Work, WorkId};

/// One work item with what agentd keeps beside it.
#[derive(Debug)]
pub struct Entry {
    /// The wire object.
    pub work: Work,
    /// The workspace it belongs to.
    pub workspace: String,
    /// The latest heartbeat as handed to the worker.
    pub last_heartbeat: Option<String>,
    /// When the lease runs out (a monotonic deadline).
    pub lease_until: Option<tokio::time::Instant>,
    /// Hosted work: its sandbox's own worker takes it, never a poller.
    pub hosted: bool,
    /// Whether a poll handed it out (with its secret) and when.
    pub handed_out: Option<tokio::time::Instant>,
}

impl Entry {
    /// A new entry for `work`.
    pub fn new(work: Work, workspace: String, hosted: bool) -> Self {
        Self { work, workspace, last_heartbeat: None, lease_until: None, hosted, handed_out: None }
    }
}

/// Every work item, by id and by session.
#[derive(Debug, Default)]
pub struct Board {
    entries: Mutex<HashMap<WorkId, Entry>>,
    by_session: Mutex<HashMap<String, WorkId>>,
    /// Wakes pollers when work is queued.
    pub queued: Notify,
}

impl Board {
    /// Add `entry`.
    pub fn insert(&self, entry: Entry) {
        self.by_session.lock().insert(entry.work.data.id.clone(), entry.work.id.clone());
        self.entries.lock().insert(entry.work.id.clone(), entry);
        self.queued.notify_waiters();
    }

    /// Run `f` on work `id`.
    pub fn with<T>(&self, id: &WorkId, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        self.entries.lock().get_mut(id).map(f)
    }

    /// Run `f` on every entry.
    pub fn each<T>(&self, mut f: impl FnMut(&mut Entry) -> Option<T>) -> Vec<T> {
        self.entries.lock().values_mut().filter_map(&mut f).collect()
    }

    /// The work item of `session`.
    pub fn of_session(&self, session: &str) -> Option<WorkId> {
        self.by_session.lock().get(session).cloned()
    }

    /// Drop `session`'s work.
    pub fn remove_session(&self, session: &str) {
        if let Some(id) = self.by_session.lock().remove(session) {
            self.entries.lock().remove(&id);
        }
    }
}
