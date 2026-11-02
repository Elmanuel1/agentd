//! The work items and their queued sandbox operations, in this process.
//! Like the event stream's broadcast, the board is per replica: the loop
//! that queues an operation and the worker that takes it meet here.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use parking_lot::Mutex;
use sandbox_protocol::{Envelope, Outcome};
use tokio::sync::{oneshot, Notify};

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
    /// Operations waiting for the worker.
    queue: VecDeque<Envelope>,
    /// Operations taken and waiting for their outcome.
    waiting: HashMap<String, oneshot::Sender<Outcome>>,
    /// Wakes a worker waiting for an operation.
    ready: Arc<Notify>,
}

impl Entry {
    /// A new entry for `work`.
    pub fn new(work: Work, workspace: String, hosted: bool) -> Self {
        Self {
            work,
            workspace,
            last_heartbeat: None,
            lease_until: None,
            hosted,
            handed_out: None,
            queue: VecDeque::new(),
            waiting: HashMap::new(),
            ready: Arc::new(Notify::new()),
        }
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

    /// Drop `session`'s work, failing what it still owes.
    pub fn remove_session(&self, session: &str) {
        if let Some(id) = self.by_session.lock().remove(session) {
            self.entries.lock().remove(&id);
        }
    }

    /// Queue `op` for work `id`; its outcome arrives on the receiver.
    pub fn enqueue(&self, id: &WorkId, op: Envelope) -> Option<oneshot::Receiver<Outcome>> {
        self.with(id, |e| {
            let (tx, rx) = oneshot::channel();
            e.waiting.insert(op.id.clone(), tx);
            e.queue.push_back(op);
            e.ready.notify_one();
            rx
        })
    }

    /// The next operation for work `id`, waiting up to `block` for one.
    pub async fn next(&self, id: &WorkId, block: std::time::Duration) -> Option<Envelope> {
        let ready = self.with(id, |e| e.ready.clone())?;
        let deadline = tokio::time::Instant::now() + block;
        loop {
            let notified = ready.notified();
            if let Some(op) = self.with(id, |e| e.queue.pop_front()).flatten() {
                return Some(op);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    /// Hand `outcome` to whoever queued operation `op` of work `id`.
    pub fn complete(&self, id: &WorkId, op: &str, outcome: Outcome) -> bool {
        self.with(id, |e| e.waiting.remove(op)).flatten().is_some_and(|tx| tx.send(outcome).is_ok())
    }
}
