//! What stops at SIGTERM and what agentd waits for: one signal every
//! background loop watches, and the tasks that drain after it, all inside one
//! deadline (`AGENTD_SHUTDOWN_GRACE_MS`), so the total is the grace, not a sum.

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// The stop signal and the tasks that finish after it.
#[derive(Debug)]
pub struct Shutdown {
    stop: watch::Sender<bool>,
    draining: Vec<JoinHandle<()>>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self { stop: watch::channel(false).0, draining: Vec::new() }
    }
}

impl Shutdown {
    /// What a loop watches: it turns true at SIGTERM.
    pub fn signal(&self) -> watch::Receiver<bool> {
        self.stop.subscribe()
    }

    /// Wait for `task` at shutdown, until the deadline.
    pub fn drain(&mut self, task: JoinHandle<()>) {
        self.draining.push(task);
    }

    /// Signal every loop, then wait for the draining tasks until `deadline`; any still running is aborted.
    pub async fn run(mut self, deadline: Instant) {
        self.stop.send_replace(true);
        let all = futures::future::join_all(self.draining.iter_mut());
        if tokio::time::timeout_at(deadline, all).await.is_err() {
            tracing::warn!("shutdown: the grace ended with work still running; it is stopped");
            self.draining.iter().for_each(JoinHandle::abort);
        }
    }
}
