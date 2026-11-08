//! What a session's harness reports, recorded with its effects: a status
//! event moves the session's status, a model request's usage adds to the
//! session's usage, a usage snapshot records its list cost, and a grader
//! verdict updates its outcome evaluations. It reports them on
//! `POST …/events` with the session's own work token.

use serde::Deserialize;
use shared::{ApiError, AuthContext, Usage, WorkspaceId};

use super::{parse_id, HarnessContract, SessionsService};
use crate::events::{Event, EventPayload, ModelUsage, SessionEvent, SpanEvent};
use crate::types::{OutcomeEvaluation, SessionId, SessionStatus, ThreadId};

fn usage(u: &ModelUsage) -> Usage {
    Usage {
        cache_creation: None,
        cache_read_input_tokens: u.cache_read_input_tokens,
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        list_cost: None,
    }
}

/// One event a session's harness reports: the wire payload and the thread it
/// belongs to (the primary thread when absent).
#[derive(Debug, Deserialize)]
pub struct Reported {
    #[serde(default)]
    pub session_thread_id: Option<ThreadId>,
    #[serde(flatten)]
    pub payload: EventPayload,
}

impl SessionsService {
    /// `POST …/events` from the session's own work token: once the batch holds
    /// no client event, the client events waiting for the agent are stamped
    /// processed, since the agent has taken them, and each reported event is
    /// recorded.
    pub async fn report(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        events: Vec<Reported>,
    ) -> Result<Vec<Event>, ApiError> {
        let session = parse_id(raw_id)?;
        if events.iter().any(|e| e.payload.is_user_event()) {
            return Err(ApiError::invalid_request("a harness sends agent, span and session events only"));
        }
        self.drain_user_events(&ctx.workspace, &session).await?;
        let mut recorded = Vec::with_capacity(events.len());
        for e in events {
            recorded
                .push(self.record(&ctx.workspace, &session, e.session_thread_id.as_ref(), e.payload).await?);
        }
        Ok(recorded)
    }

    /// Record `payload`, which the session's harness sent for `thread` (the
    /// primary thread when `None`).
    async fn record(
        &self,
        ws: &WorkspaceId,
        session: &SessionId,
        thread: Option<&ThreadId>,
        mut payload: EventPayload,
    ) -> Result<Event, ApiError> {
        let status = match (&payload, thread) {
            (EventPayload::Session(SessionEvent::StatusRunning {}), None) => Some(SessionStatus::Running),
            (EventPayload::Session(SessionEvent::StatusIdle { .. }), None) => Some(SessionStatus::Idle),
            (EventPayload::Session(SessionEvent::StatusTerminated {}), None) => {
                Some(SessionStatus::Terminated)
            }
            _ => None,
        };
        if let Some(status) = status {
            self.mutate(ws, session, |s| {
                if s.status == SessionStatus::Terminated {
                    return Err(ApiError::conflict_invalid_request("session is terminated"));
                }
                s.status = status;
                Ok(())
            })
            .await?;
        }
        match &payload {
            EventPayload::Span(SpanEvent::ModelRequestEnd { model_usage, .. }) => {
                self.add_usage(ws, session, &usage(model_usage)).await?;
            }
            EventPayload::Span(SpanEvent::OutcomeEvaluationEnd {
                outcome_id,
                result,
                explanation,
                iteration,
                usage: grader,
                ..
            }) => {
                self.add_usage(ws, session, &usage(grader)).await?;
                let eval = OutcomeEvaluation {
                    outcome_id: outcome_id.clone(),
                    result: *result,
                    explanation: explanation.clone(),
                    iteration: *iteration,
                };
                self.record_outcome(ws, session, eval).await?;
            }
            EventPayload::Session(SessionEvent::Usage { usage: reported, .. }) => {
                let reported = reported.list_cost.clone();
                // The snapshot shows the session's own usage and budget, not the harness's view.
                let s = self.mutate(ws, session, |s| s.usage.record_list_cost(reported.clone())).await?;
                payload = SessionEvent::Usage { usage: s.usage, budget: s.budget }.into();
            }
            _ => {}
        }
        let event = self.emit(ws, session, thread, payload).await?;
        if status == Some(SessionStatus::Terminated) {
            self.stop_sandbox(session).await;
        }
        Ok(event)
    }
}
