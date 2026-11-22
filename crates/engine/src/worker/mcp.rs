//! A step that calls the MCP tools the model chose, with no model call: it
//! records each result as its call finishes, as live does (MC-26's capture),
//! and stays due, so the next step calls the model with them (MC-25).

use sessions::events::{AgentEvent, Event, EventId, SessionEvent};
use sessions::steps::{Claim, StepEnd};
use sessions::types::{Session, SessionStatus};
use shared::BackendError;

use super::Loop;
use super::take::Taken;
use super::turn::batch;
use crate::mcp::CallOutcome;
use crate::turn::policy::model_name;

impl Loop {
    /// Call each allowed MCP call the step's thread has unanswered and write
    /// its result before the next call goes out. A session idle when the
    /// step starts (a confirmation allowed the call) runs again, and its
    /// servers connect again as at any turn's start.
    pub(super) async fn call_mcp(
        &self,
        claim: &Claim,
        session: Session,
        step: Taken,
    ) -> Result<(), BackendError> {
        let Taken { taken, answers, thread, history, read, .. } = step;
        let turn = thread.turn();
        let starting = session.status == SessionStatus::Idle;
        let mut servers = self.stores.mcp.servers(&claim.workspace, &session, &thread.agent, &read).await?;
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
        let opening = batch(taken, running, events);
        let empty = opening.read.is_empty() && opening.events.is_empty();
        if !empty && !self.write(claim, &opening, Some(SessionStatus::Running)).await? {
            return Ok(());
        }
        let mut finished: Vec<Event> = Vec::new();
        for call in history.unanswered.iter().filter(|u| u.for_loop()) {
            let Ok(id) = EventId::parse(&call.id) else { continue };
            let done = std::mem::take(&mut finished);
            if !done.is_empty() && !self.write(claim, &batch(Vec::new(), Vec::new(), done), None).await? {
                return Ok(());
            }
            let server = call.mcp.as_deref().unwrap_or_default();
            let prefix = model_name(server, "");
            let tool = call.name.strip_prefix(&prefix).unwrap_or(&call.name);
            let (content, is_error, error) = match servers.call(server, tool, &call.input).await? {
                CallOutcome::Called(called) => {
                    crate::metrics::mcp_call(match called.is_error {
                        true => "tool_error",
                        false => "ok",
                    });
                    (called.content, called.is_error, None)
                }
                // Another worker refreshes the token: what ran is kept, the
                // rest runs in a later step.
                CallOutcome::Deferred => return self.defer_refresh(claim).await,
                CallOutcome::Failed { content, error } => {
                    crate::metrics::mcp_call("unreachable");
                    (content, true, error)
                }
            };
            let error = error.map(|error| turn.event(SessionEvent::Error { error }));
            let result = turn.event(AgentEvent::McpToolResult { mcp_tool_use_id: id, content, is_error });
            finished = error.into_iter().chain([result]).collect();
        }
        self.commit(claim, StepEnd::Again, &batch(Vec::new(), Vec::new(), finished), None).await?;
        Ok(())
    }

    /// Release the step unrun, due again in a second, while another worker
    /// refreshes a credential's token (MC-20, MC-23).
    pub(super) async fn defer_refresh(&self, claim: &Claim) -> Result<(), BackendError> {
        self.stores.steps.defer(claim, 1).await.map_err(BackendError::new)
    }
}
