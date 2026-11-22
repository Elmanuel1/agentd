//! What a step reads of its session's log.

use sessions::steps::Claim;
use sessions::store::EngineEvents;
use sessions::types::primary_thread_id;
use shared::BackendError;

use super::Loop;
use super::usage::KEPT;

impl Loop {
    /// The session's own step reads the whole log; a thread's step reads the
    /// thread's events and what it needs of the rest.
    pub(super) async fn read(&self, claim: &Claim) -> Result<EngineEvents, BackendError> {
        let (ws, id) = (&claim.workspace, &claim.session);
        let read = match &claim.thread {
            None => self.stores.log.engine_events(ws, id).await,
            Some(t) => {
                let scope = (*t != primary_thread_id(id)).then_some(t);
                self.stores.log.thread_events(ws, id, scope).await
            }
        };
        read.map_err(BackendError::new)
    }

    /// The session's exact list cost so far, in billionths of a dollar:
    /// every thread's calls.
    pub(super) async fn cost_so_far(&self, claim: &Claim) -> Result<u64, BackendError> {
        let sum = self.stores.log.engine_sum(&claim.workspace, &claim.session, KEPT).await;
        sum.map_err(BackendError::new)
    }
}
