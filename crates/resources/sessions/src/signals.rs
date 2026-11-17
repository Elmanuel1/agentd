//! Signals that a session's log or row changed, so each replica's stream of
//! that session reads the log again. A signal carries no event: the log is
//! the one source, and a lost signal only delays a read to the fallback
//! timer.

use shared::ports::BoxFuture;
use shared::{BackendError, WorkspaceId};
use std::future::Future;
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
pub trait Signals: Send + Sync {
    /// Tell every replica `session` changed.
    fn publish<'a>(&'a self, signal: &'a Signal) -> BoxFuture<'a, Result<(), BackendError>>;
    /// The signals every replica publishes, this one's included. A receiver
    /// that falls behind sees `Lagged`, not the signals it missed.
    fn subscribe(&self) -> broadcast::Receiver<Signal>;
}

/// What an adapter implements to provide [`Signals`]; the methods are documented there.
pub trait NativeSignals: Send + Sync {
    fn publish(&self, signal: &Signal) -> impl Future<Output = Result<(), BackendError>> + Send;
    fn subscribe(&self) -> broadcast::Receiver<Signal>;
}

impl<T: NativeSignals> Signals for T {
    fn publish<'a>(&'a self, signal: &'a Signal) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(NativeSignals::publish(self, signal))
    }
    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        NativeSignals::subscribe(self)
    }
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

impl NativeSignals for LocalSignals {
    async fn publish(&self, signal: &Signal) -> Result<(), BackendError> {
        // No receiver is not an error: nothing streams that session here.
        let _ = self.0.send(signal.clone());
        Ok(())
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.0.subscribe()
    }
}
