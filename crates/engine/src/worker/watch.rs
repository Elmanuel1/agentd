//! While a step's model call runs, its session's signals are heard: a
//! `user.interrupt` in the log cancels the call, as live does.

use std::sync::Arc;

use sessions::events::{EventId, EventPayload, UserEvent};
use sessions::signals::Signal;
use sessions::store::SessionEventLog;
use shared::WorkspaceId;
use tokio::sync::broadcast::{self, error::RecvError};

use sessions::types::SessionId;

/// A session's signals during one model call.
pub(super) struct Watch {
    pub(super) rx: broadcast::Receiver<Signal>,
    pub(super) log: Arc<dyn SessionEventLog>,
    pub(super) ws: WorkspaceId,
    pub(super) id: SessionId,
}

impl Watch {
    /// Resolve with the interrupts the client sent once there is one; until
    /// then, pend. The log is read first, for an interrupt sent before the
    /// watch subscribed, and again on each signal or missed signal.
    pub(super) async fn interrupted(&mut self) -> Vec<EventId> {
        let mut heard = false;
        loop {
            if std::mem::replace(&mut heard, true) {
                match self.rx.recv().await {
                    Ok(s) if s.session != self.id => continue,
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return std::future::pending().await,
                }
            }
            let queued = match self.log.unprocessed_events(&self.ws, &self.id).await {
                Ok(queued) => queued,
                Err(e) => {
                    tracing::warn!(session = %self.id, error = %e, "loop: could not read the log for interrupts");
                    continue;
                }
            };
            let interrupts: Vec<EventId> = queued
                .into_iter()
                .filter(|e| matches!(e.payload, EventPayload::User(UserEvent::Interrupt {})))
                .map(|e| e.id)
                .collect();
            if !interrupts.is_empty() {
                return interrupts;
            }
        }
    }
}
