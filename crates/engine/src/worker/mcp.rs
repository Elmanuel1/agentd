//! A step that calls the MCP tools the model chose, with no model call: it
//! commits each result and stays due, so the next step calls the model with
//! them (MC-25).

use sessions::events::{AgentEvent, Event, EventId, SessionEvent};
use sessions::steps::{Claim, StepEnd};
use sessions::types::{Session, SessionStatus};
use shared::BackendError;

use super::team::Thread;
use super::turn::batch;
use super::Loop;
use crate::mcp::CallOutcome;
use crate::turn::history::Unanswered;

impl Loop {
    /// Call each allowed MCP call in `unanswered` and commit its result. A
    /// session idle when the step starts (a confirmation allowed the call)
    /// runs again, and its servers connect again as at any turn's start.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn call_mcp(
        &self,
        claim: &Claim,
        mut session: Session,
        thread: &Thread,
        log: &[Event],
        taken: Vec<EventId>,
        answers: Vec<Event>,
        unanswered: &[Unanswered],
    ) -> Result<(), BackendError> {
        let turn = thread.turn();
        let starting = session.status == SessionStatus::Idle;
        let mut servers = self.stores.mcp.servers(&claim.workspace, &session, &thread.agent, log).await?;
        let mut events = answers;
        let running = match starting {
            true => turn.running(),
            false => Vec::new(),
        };
        if starting {
            let errors = servers.connect(true).await?;
            events.extend(errors.into_iter().map(|error| turn.event(SessionEvent::Error { error })));
        }
        if servers.deferred() {
            return self.defer_refresh(claim).await;
        }
        let mut called_any = false;
        for call in unanswered.iter().filter(|u| u.for_loop()) {
            let Ok(id) = EventId::parse(&call.id) else { continue };
            let server = call.mcp.as_deref().unwrap_or_default();
            let (content, is_error) = match servers.call(server, &call.name, &call.input).await? {
                CallOutcome::Called(called) => {
                    crate::metrics::mcp_call(if called.is_error { "tool_error" } else { "ok" });
                    (called.content, called.is_error)
                }
                // Another worker refreshes the token: what ran is kept, the
                // rest runs in a later step.
                CallOutcome::Deferred if called_any => break,
                CallOutcome::Deferred => return self.defer_refresh(claim).await,
                CallOutcome::Failed { content, error } => {
                    crate::metrics::mcp_call("unreachable");
                    events.push(turn.event(SessionEvent::Error { error }));
                    (content, true)
                }
            };
            events.push(turn.event(AgentEvent::McpToolResult { mcp_tool_use_id: id, content, is_error }));
            called_any = true;
        }
        session.status = SessionStatus::Running;
        self.commit(claim, StepEnd::Again, &batch(taken, running, events), &session).await?;
        Ok(())
    }

    /// Release the step unrun, due again in a second, while another worker
    /// refreshes a credential's token (MC-20, MC-23).
    pub(super) async fn defer_refresh(&self, claim: &Claim) -> Result<(), BackendError> {
        self.stores.steps.defer(claim, 1).await.map_err(BackendError::new)
    }
}
