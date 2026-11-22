//! What a step reads of its session's log.

use sessions::events::{AgentEvent, EventPayload, SpanEvent};
use sessions::steps::Claim;
use sessions::store::{EngineEvents, ThreadRead};
use sessions::types::{ThreadId, primary_thread_id};
use shared::BackendError;

use super::Loop;
use super::grade::grader;

impl Loop {
    /// Each thread from its latest checkpoint for the session's own step;
    /// for a thread's step, the thread's events and what it needs of the rest.
    pub(super) async fn read(&self, claim: &Claim) -> Result<EngineEvents, BackendError> {
        let (ws, id) = (&claim.workspace, &claim.session);
        let Some(t) = &claim.thread else {
            return self.stores.log.step_events(ws, id).await.map_err(BackendError::new);
        };
        let scope = (*t != primary_thread_id(id)).then_some(t);
        let log = &self.stores.log;
        let read = log.thread_events(ws, id, scope, ThreadRead { whole: false }).await;
        let read = read.map_err(BackendError::new)?;
        if !compacted_grader(&read, t) {
            return Ok(read);
        }
        // A grader's stall count spans its round: it reads its round whole.
        log.thread_events(ws, id, scope, ThreadRead { whole: true }).await.map_err(BackendError::new)
    }
}

/// `thread` grades a round and has compacted since it began.
fn compacted_grader(read: &EngineEvents, thread: &ThreadId) -> bool {
    let grades = read.iter().any(|(e, _)| {
        matches!(e.payload, EventPayload::Span(SpanEvent::OutcomeEvaluationStart { .. }))
            && grader(&e.id).as_ref() == Some(thread)
    });
    grades
        && read.iter().any(|(e, _)| {
            e.session_thread_id.as_ref() == Some(thread)
                && matches!(e.payload, EventPayload::Agent(AgentEvent::ThreadContextCompacted {}))
        })
}
