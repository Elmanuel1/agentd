//! Starts a session's loop when its client events arrive: one loop per
//! session at a time, across replicas, under the session's lease.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use sessions::events::{SessionEvent, StopReason};
use sessions::types::{SessionId, SessionStatus};
use sessions::TurnTrigger;
use shared::ports::lease::Leases;
use shared::{ApiError, AuthContext, WorkspaceId};
use tokio::sync::Notify;

use super::Engine;

/// The [`TurnTrigger`] that runs the engine.
#[derive(Clone)]
pub struct Dispatcher {
    engine: Arc<Engine>,
    leases: Arc<dyn Leases>,
    /// The loops this process runs, by session; waking one nudges it.
    active: Arc<Mutex<HashMap<SessionId, Arc<Notify>>>>,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").field("active", &self.active.lock().len()).finish_non_exhaustive()
    }
}

impl Dispatcher {
    /// Run `engine` for woken sessions, one holder per session through `leases`.
    pub fn new(engine: Arc<Engine>, leases: Arc<dyn Leases>) -> Self {
        Self { engine, leases, active: Arc::default() }
    }

    /// Whether this process is running `session`'s loop.
    pub fn is_running(&self, session: &SessionId) -> bool {
        self.active.lock().contains_key(session)
    }

    /// Say why the loop stopped and leave the session idle.
    async fn fail(&self, ws: &WorkspaceId, id: &SessionId, e: &ApiError) {
        tracing::error!(session = %id, error = %e.message, "engine: the session loop failed");
        let error = serde_json::json!({"type": "engine_error", "message": e.message});
        let contract = &self.engine.contract;
        let _ = contract.emit(ws, id, None, SessionEvent::Error { error }.into()).await;
        let stop = Some(StopReason::RetriesExhausted {});
        let _ = contract.set_status(ws, id, SessionStatus::Idle, stop).await;
    }

    /// Run the session's loop while it has client events to take. A loop
    /// another replica holds the lease for is left to that replica, which
    /// reads the new events itself.
    async fn drive(self, ctx: AuthContext, id: SessionId, wake: Arc<Notify>) {
        let ws = ctx.workspace.clone();
        let key = format!("session:{id}");
        loop {
            match self.leases.try_acquire(&key).await {
                Ok(Some(lease)) => {
                    let failed = self.engine.run_session(&ctx, &id, wake.clone()).await.err();
                    if let Some(e) = &failed {
                        self.fail(&ws, &id, e).await;
                    }
                    drop(lease);
                    // The events that failed it stay queued for the next wake.
                    if failed.is_some() {
                        self.active.lock().remove(&id);
                        return;
                    }
                }
                // Its holder takes the new events before it lets go.
                Ok(None) => {
                    self.active.lock().remove(&id);
                    return;
                }
                Err(e) => {
                    tracing::error!(session = %id, error = %e, "engine: could not take the session lease")
                }
            }
            // Leave the active map before the last look, so a wake that comes
            // after it starts a loop of its own instead of nudging this one.
            self.active.lock().remove(&id);
            if !self.engine.contract.has_unprocessed(&ws, &id).await.unwrap_or(false)
                || self.active.lock().contains_key(&id)
            {
                return;
            }
            self.active.lock().insert(id.clone(), wake.clone());
        }
    }
}

impl TurnTrigger for Dispatcher {
    fn wake(&self, ctx: &AuthContext, session: &SessionId) {
        let mut active = self.active.lock();
        if let Some(notify) = active.get(session) {
            notify.notify_one();
            return;
        }
        let wake = Arc::new(Notify::new());
        active.insert(session.clone(), wake.clone());
        drop(active);
        tokio::spawn(self.clone().drive(ctx.clone(), session.clone(), wake));
    }
}
