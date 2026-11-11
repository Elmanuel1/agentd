//! One step: read the session's log, take the client events it holds, call
//! the model once with no transaction open, and commit what the reply leads
//! to. As on live, the session reads `running` before the call: the start of
//! the call is written first, with the client events it takes.

use std::collections::HashSet;

use sessions::events::{Event, EventId, SpanEvent, StopReason};
use sessions::signals::Signal;
use sessions::steps::{Claim, StepEnd, StepWrite};
use sessions::store::StoreError;
use sessions::types::{Session, SessionStatus};
use shared::tenancy::RUNTIME_QUEUE;
use shared::BackendError;

use super::reply::{led, Call};
use super::settle::settle;
use super::take::{cut_off, take, Interrupt, Taken};
use super::team::roster;
use super::turn::{batch, opening, Batch};
use super::usage::{charged, over_budget, spent};
use super::watch::Watch;
use super::Loop;
use super::{grade, rubrics};
use crate::model::{ModelError, ModelProvider, ModelRequest};
use crate::turn::policy::Toolbox;
use crate::turn::prompt::system_prompt;

impl Loop {
    /// Run `claim`'s step; a step that fails goes back on the queue for a
    /// retry after a pause.
    pub(super) async fn step(&self, claim: Claim) {
        let Err(e) = self.run(&claim).await else { return };
        tracing::warn!(session = %claim.session, error = %e, "loop: the step failed; it runs again");
        if let Err(e) = self.stores.steps.defer(&claim, 5).await {
            tracing::warn!(session = %claim.session, error = %e, "loop: could not release the step");
        }
    }

    async fn write(&self, claim: &Claim, b: &Batch, to: Option<SessionStatus>) -> Result<bool, BackendError> {
        let change = move |s: &mut Session| to.filter(|t| s.status != *t).map(|t| s.status = t).is_some();
        let w = StepWrite { read: &b.read, at: b.at, events: &b.events, session: &change };
        let wrote = self
            .stores
            .steps
            .write(claim, self.settings.lease_seconds(), w)
            .await
            .map_err(BackendError::new)?;
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
            s.outcome_evaluations = after.outcome_evaluations.clone();
            true
        };
        let w = StepWrite { read: &b.read, at: b.at, events: &b.events, session: &change };
        let done = self.stores.steps.commit(claim, end, w).await.map_err(BackendError::new)?;
        self.publish(&Signal { workspace: claim.workspace.clone(), session: claim.session.clone() }).await;
        Ok(done)
    }

    async fn run(&self, claim: &Claim) -> Result<(), BackendError> {
        let (ws, id) = (&claim.workspace, &claim.session);
        let mut session = match self.stores.sessions.get(ws, id).await {
            Ok(session) => session,
            // Gone: the delete took its step with it.
            Err(StoreError::NotFound) => return Ok(()),
            Err(e) => return Err(BackendError::new(e)),
        };
        let log = self.stores.log.events(ws, id).await.map_err(BackendError::new)?;
        let queue = self.stores.work.queue_of(ws, id.as_str()).await.map_err(BackendError::new)?;
        let cloud = queue.is_none_or(|q| q == RUNTIME_QUEUE);
        let idle = session.status == SessionStatus::Idle;
        let rubrics = rubrics::read(&self.stores.files, ws, &log).await?;
        let Taken { taken, arrived, answers, allowed, thread, history, read, interrupt } =
            take(&log, &session, cloud, &rubrics);
        let turn = thread.turn();
        if interrupt == Interrupt::Second {
            self.stores.work.halt(ws, id.as_str()).await.map_err(BackendError::new)?;
        }
        if interrupt != Interrupt::None {
            session.status = SessionStatus::Idle;
            let mut events = cut_off(&turn, &history);
            events.extend(turn.idle(&session, StopReason::EndTurn {}));
            self.commit(claim, StepEnd::Done, &batch(taken, Vec::new(), events), &session).await?;
            return Ok(());
        }
        if !history.awaits_model() {
            let end = match allowed {
                true => StepEnd::Requeue,
                false => StepEnd::Done,
            };
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
        let provider = ModelProvider::of(&model).name();
        if !self.admitted(claim, provider).await? {
            return Ok(());
        }
        let start = turn.event(SpanEvent::ModelRequestStart {});
        let start_id = start.id.clone();
        let mut running = match idle {
            true => turn.running(),
            false => Vec::new(),
        };
        let ongoing =
            thread.grading.as_ref().map(|o| SpanEvent::OutcomeEvaluationOngoing { outcome_id: o.id.clone() });
        running.extend(ongoing.map(|span| Event { session_thread_id: None, ..turn.event(span) }));
        let opening = opening(taken, arrived, running, answers.into_iter().chain([start]).collect());
        if !self.write(claim, &opening, Some(SessionStatus::Running)).await? {
            return Ok(());
        }
        session.status = SessionStatus::Running;
        let roster = roster(&session);
        let coordinator = thread.primary && !roster.is_empty();
        let delegates = match coordinator {
            true => roster.as_slice(),
            false => &[],
        };
        let tools = match &thread.grading {
            Some(_) => grade::tools(),
            None => Toolbox::for_agent(&agent, coordinator),
        };
        let effort = agent
            .config
            .model
            .effort
            .as_ref()
            .and_then(|e| serde_json::to_value(e).ok()?.as_str().map(str::to_owned));
        let request = ModelRequest {
            model,
            agent: agent.config.name.clone(),
            system: system_prompt(&agent, &[], delegates),
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
        let failed = |e: Option<&ModelError>| turn.failed(&start_id, e);
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
                    // The grader's built-in calls never idle the session (L-5).
                    cloud: cloud || thread.grading.is_some(),
                    more,
                    session: &session,
                    log: &read,
                    grading: thread.grading.as_ref(),
                };
                let mut out = led(&call, &reply);
                settle(&mut out, &turn, &thread, &mut session, &read, &reply);
                if let Some(eval) = out.evaluation.take() {
                    session.outcome_evaluations.retain(|e| e.outcome_id != eval.outcome_id);
                    session.outcome_evaluations.push(eval);
                }
                (batch(Vec::new(), Vec::new(), out.events), out.end)
            }
        };
        self.commit(claim, end, &done, &session).await?;
        Ok(())
    }
}
