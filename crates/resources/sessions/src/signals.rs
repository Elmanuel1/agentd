//! Signals that a session's log or row changed, so each replica's stream of
//! that session reads the log again. A signal carries no event: the log is
//! the one source, and a lost signal only delays a read to the fallback
//! timer.

use async_trait::async_trait;
use shared::{BackendError, WorkspaceId};
use tokio::sync::broadcast;

use crate::types::SessionId;

/// One session changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Signal {
    /// The session's workspace.
    pub workspace: WorkspaceId,
    /// The session.
    pub session: SessionId,
}

/// Every replica's signals, in and out.
#[async_trait]
pub trait Signals: Send + Sync {
    /// Tell every replica `session` changed.
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError>;
    /// The signals every replica publishes, this one's included. A receiver
    /// that falls behind sees `Lagged`, not the signals it missed.
    fn subscribe(&self) -> broadcast::Receiver<Signal>;
}

/// The signals of one process, for a single replica.
#[derive(Debug, Clone)]
pub struct LocalSignals(broadcast::Sender<Signal>);

impl Default for LocalSignals {
    fn default() -> Self {
        Self(broadcast::channel(CAPACITY).0)
    }
}

/// Signals a replica holds before a slow reader lags.
pub const CAPACITY: usize = 4096;

#[async_trait]
impl Signals for LocalSignals {
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError> {
        // No receiver is not an error: nothing streams that session here.
        let _ = self.0.send(signal.clone());
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.0.subscribe()
    }
}
