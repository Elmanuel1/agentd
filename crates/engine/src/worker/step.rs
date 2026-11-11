//! One step: read the session's log, take the client events it holds, call
//! the model once with no transaction open, and commit what the reply leads
//! to. As on live, the session reads `running` before the call: the start of
//! the call is written first, with the client events it takes.

use std::collections::HashSet;

use sessions::events::{EventId, SessionEvent, SpanEvent, StopReason};
use sessions::signals::Signal;
use sessions::steps::{Claim, StepEnd, StepWrite};
use sessions::types::{Session, SessionStatus};
use shared::tenancy::RUNTIME_QUEUE;
use shared::{BackendError, Timestamp};
use work::store::Reach;

use super::reply::{led, Call};
use super::take::{cut_off, take, Interrupt, Taken};
use super::team::settle;
use super::turn::{batch, Batch};
use super::usage::{charged, over_budget, spent};
use super::watch::Watch;
use super::Loop;
use crate::model::budget::Admit;
use crate::model::{ModelError, ModelRequest};
use crate::turn::policy::Toolbox;
use crate::turn::prompt::system_prompt;

fn lease(l: &Loop) -> i64 {
    i64::try_from(l.settings.lease.as_secs()).unwrap_or(i64::MAX)
}

impl Loop {
    /// Run `claim`'s step; a step that fails goes back on the queue for a
    /// retry after a pause.
    pub(super) async fn step(&self, claim: Claim) {
        if let Err(e) = self.run(&claim).await {
            tracing::warn!(session = %claim.session, error = %e, "loop: the step failed; it runs again");
            if let Err(e) = self.stores.steps.defer(&claim, 5).await {
                tracing::warn!(session = %claim.session, error = %e, "loop: could not release the step");
            }
        }
    }

    async fn write(&self, claim: &Claim, b: &Batch, to: Option<SessionStatus>) -> Result<bool, BackendError> {
        let change = move |s: &mut Session| to.filter(|t| s.status != *t).map(|t| s.status = t).is_some();
        let w = StepWrite { read: &b.read, at: b.at, events: &b.events, session: &change };
        let wrote = self.stores.steps.write(claim, lease(self), w).await.map_err(BackendError::new)?;
        self.publish(&Signal { workspace: claim.workspace.clone(), session: claim.session.clone() }).await;
        Ok(wrote)
    }

    async fn commit(
        &self,
        claim: &Claim,
        end: StepEnd,
        b: &Batch,
        session: &Session,
    ) -> Result<bool, BackendError> {
        let after = session.clone();
        let change = move |s: &mut Session| {
            s.status = after.status;
            s.usage = after.usage.clone();
            true
        };
        let w = StepWrite { read: &b.read, at: b.at, events: &b.events, session: &change };
        let done = self.stores.steps.commit(claim, end, w).await.map_err(BackendError::new)?;
        self.publish(&Signal { workspace: claim.workspace.clone(), session: claim.session.clone() }).await;
        Ok(done)
    }

    async fn run(&self, claim: &Claim) -> Result<(), BackendError> {
        let (ws, id) = (&claim.workspace, &claim.session);
        let Ok(mut session) = self.stores.sessions.get(ws, id).await else {
            // Gone: the delete took its step with it.
            return Ok(());
        };
        let log = self.stores.log.events(ws, id).await.map_err(BackendError::new)?;
        let item =
            self.stores.work.get(ws, id.as_str(), &Reach::default()).await.map_err(BackendError::new)?;
        let cloud = item.as_ref().is_none_or(|i| i.queue == RUNTIME_QUEUE);
        let idle = session.status == SessionStatus::Idle;
        let Taken { taken, answers, allowed, thread, history, interrupt } = take(&log, &session, cloud);
        let turn = thread.turn();
        if interrupt != Interrupt::None {
            if interrupt == Interrupt::Second {
                let stopped =
                    self.stores.work.stop(ws, id.as_str(), &Reach::default(), true, Timestamp::now()).await;
                stopped.map_err(BackendError::new)?;
            }
            session.status = SessionStatus::Idle;
            let mut events = cut_off(&turn, &history);
            events.extend(turn.idle(&session, StopReason::EndTurn {}));
            self.commit(claim, StepEnd::Done, &batch(taken, Vec::new(), events), &session).await?;
            return Ok(());
        }
        if !history.awaits_model() {
            let end = if allowed { StepEnd::Requeue } else { StepEnd::Done };
            self.commit(claim, end, &batch(taken, Vec::new(), answers), &session).await?;
            return Ok(());
        }
        if over_budget(&session) {
            session.status = SessionStatus::Idle;
            let mut events = answers;
            events.extend(turn.idle(&session, StopReason::BudgetReached {}));
            self.commit(claim, StepEnd::Done, &batch(taken, Vec::new(), events), &session).await?;
            return Ok(());
        }
        let agent = thread.agent.clone();
        let model = agent.config.model.id.clone();
        let provider = if model.contains('/') { "openrouter" } else { "anthropic" };
        match self.models.budget.admit(provider).await {
            Ok(Admit::Yes) => {}
            Ok(Admit::Later(seconds)) => {
                return self.stores.steps.defer(claim, seconds).await.map_err(BackendError::new)
            }
            Err(e) => {
                tracing::warn!(session = %id, error = %e, "loop: the model budget cannot be read; the step waits");
                return self.stores.steps.defer(claim, 1).await.map_err(BackendError::new);
            }
        }
        let start = turn.event(SpanEvent::ModelRequestStart {});
        let start_id = start.id.clone();
        let running = if idle { turn.running() } else { Vec::new() };
        let opening = batch(taken, running, answers.into_iter().chain([start]).collect());
        if !self.write(claim, &opening, Some(SessionStatus::Running)).await? {
            return Ok(());
        }
        session.status = SessionStatus::Running;
        let roster = session.agent.config.multiagent.as_ref().map(|m| m.roster_agents()).unwrap_or_default();
        let coordinator = thread.primary && !roster.is_empty();
        let tools = Toolbox::for_agent(&agent, coordinator);
        let effort = agent
            .config
            .model
            .effort
            .as_ref()
            .and_then(|e| serde_json::to_value(e).ok()?.as_str().map(str::to_owned));
        let request = ModelRequest {
            model,
            agent: agent.config.name.clone(),
            system: system_prompt(&agent, &[], if coordinator { &roster } else { &[] }),
            messages: history.messages,
            tools: tools.tools.clone(),
            max_tokens: self.models.max_tokens,
            effort,
        };
        let mut watch = Watch {
            rx: self.signals.subscribe(),
            log: self.stores.log.clone(),
            ws: ws.clone(),
            id: id.clone(),
        };
        let answer = tokio::select! {
            answer = self.models.retry.call(self.models.client.as_ref(), request) => Ok(answer),
            interrupts = watch.interrupted() => Err(interrupts),
        };
        let failed = |e: Option<&ModelError>| {
            let mut events = vec![turn.event(SpanEvent::ModelRequestEnd {
                model_request_start_id: start_id.clone(),
                is_error: true,
                model_usage: Default::default(),
            })];
            if let Some(e) = e {
                events.push(turn.event(SessionEvent::Error {
                    error: serde_json::json!({"type": "model_error", "message": e.to_string()}),
                }));
            }
            events
        };
        let (done, end) = match answer {
            // As on live: the call's end, then the interrupt, then the idle.
            Err(interrupts) => {
                session.status = SessionStatus::Idle;
                (batch(interrupts, failed(None), turn.idle(&session, StopReason::EndTurn {})), StepEnd::Done)
            }
            Ok(Err(e)) => {
                session.status = SessionStatus::Idle;
                let mut events = failed(Some(&e));
                events.extend(turn.idle(&session, StopReason::RetriesExhausted {}));
                (batch(Vec::new(), Vec::new(), events), StepEnd::Done)
            }
            Ok(Ok(reply)) => {
                let (usage, cents) = spent(&reply);
                if let Err(e) = self.models.budget.spend(provider, usage.input_tokens).await {
                    tracing::warn!(session = %id, error = %e, "loop: could not count the call's tokens");
                }
                charged(&mut session, &usage, cents);
                let known: HashSet<&EventId> = log.iter().map(|e| &e.id).collect();
                let later = self.stores.log.unprocessed_events(ws, id).await.map_err(BackendError::new)?;
                let more = later.iter().any(|e| !known.contains(&e.id));
                let call = Call {
                    turn: &turn,
                    start: &start_id,
                    tools: &tools,
                    cloud,
                    more,
                    session: &session,
                    log: &log,
                };
                let mut out = led(&call, &reply);
                settle(&mut out, &turn, &thread, &mut session, &log, &reply);
                (batch(Vec::new(), Vec::new(), out.events), out.end)
            }
        };
        self.commit(claim, end, &done, &session).await?;
        Ok(())
    }
}
