//! What a step reads of its session's log.

use sessions::steps::Claim;
use sessions::store::EngineEvents;
use sessions::types::primary_thread_id;
use shared::BackendError;

use super::Loop;

impl Loop {
    /// Each thread from its latest checkpoint for the session's own step;
    /// for a thread's step, the thread's events and what it needs of the rest.
    pub(super) async fn read(&self, claim: &Claim) -> Result<EngineEvents, BackendError> {
        let (ws, id) = (&claim.workspace, &claim.session);
        let read = match &claim.thread {
            None => self.stores.log.step_events(ws, id).await,
            Some(t) => {
                let scope = (*t != primary_thread_id(id)).then_some(t);
                self.stores.log.thread_events(ws, id, scope).await
            }
        };
        read.map_err(BackendError::new)
    }
}
