//! One step: read the session's log, take the client events it holds, call
//! the model once with no transaction open, and commit what the reply leads
//! to. As on live, the session reads `running` before the call: the start of
//! the call is written first, with the client events it takes.

use std::collections::HashSet;

use sessions::events::{Event, EventId, SessionEvent, SpanEvent, StopReason};
use sessions::steps::{Claim, StepEnd};
use sessions::store::StoreError;
use sessions::types::SessionStatus;
use shared::BackendError;
use shared::tenancy::RUNTIME_QUEUE;

use super::Loop;
use super::compact::{Log, Opened};
use super::reply::{Call, led};
use super::rubrics;
use super::settle::settle;
use super::take::{Interrupt, Taken, cut_off, take};
use super::turn::{batch, opening};
use super::usage::{charged, over_budget, spent};
use super::watch::Watch;
use crate::model::{ModelError, ModelProvider};
use crate::turn::compact::checkpoint;
use crate::turn::history::{Kept, Unanswered};

impl Loop {
    /// Run `claim`'s step; a step that fails goes back on the queue for a
    /// retry after a pause.
    pub(super) async fn step(&self, claim: Claim) {
        crate::metrics::claimed(claim.waited_ms);
        let Err(e) = self.kept(&claim, self.run(&claim)).await else { return };
        tracing::warn!(session = %claim.session, error = %e, "loop: the step failed; it runs again");
        if let Err(e) = self.stores.steps.defer(&claim, 5).await {
            tracing::warn!(session = %claim.session, error = %e, "loop: could not release the step");
        }
    }

    async fn run(&self, claim: &Claim) -> Result<(), BackendError> {
        let (ws, id) = (&claim.workspace, &claim.session);
        let mut session = match self.stores.sessions.get(ws, id).await {
            Ok(session) => session,
            // Gone: the delete took its step with it.
            Err(StoreError::NotFound) => return Ok(()),
            Err(e) => return Err(BackendError::new(e)),
        };
        let read = self.stores.log.engine_events(ws, id).await.map_err(BackendError::new)?;
        crate::metrics::read(read.len());
        let kept: Kept = read.iter().filter_map(|(e, k)| Some((e.id.clone(), k.clone()?))).collect();
        let log: Vec<Event> = read.into_iter().map(|(e, _)| e).collect();
        let queue = self.stores.work.queue_of(ws, id.as_str()).await.map_err(BackendError::new)?;
        let cloud = queue.is_none_or(|q| q == RUNTIME_QUEUE);
        let idle = session.status == SessionStatus::Idle;
        let rubrics = rubrics::read(&self.stores.files, ws, &log).await?;
        let step = take(&log, &session, cloud, &rubrics, &kept);
        if step.interrupt == Interrupt::None && step.history.unanswered.iter().any(Unanswered::for_loop) {
            return self.call_mcp(claim, session, step).await;
        }
        let Taken { taken, arrived, answers, allowed, thread, history, read, interrupt } = step;
        let turn = thread.turn();
        if interrupt == Interrupt::Second {
            self.stores.work.halt(ws, id.as_str()).await.map_err(BackendError::new)?;
        }
        if interrupt != Interrupt::None {
            session.status = SessionStatus::Idle;
            let mut events = cut_off(&turn, &history, &log);
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
        let model = &agent.config.model.id;
        let provider = ModelProvider::of(model).name();
        if !self.admitted(claim, provider).await? {
            return Ok(());
        }
        let mut servers = self.stores.mcp.servers(ws, &session, &agent, &read).await?;
        let errors = servers.connect(idle).await?;
        if servers.deferred() {
            return self.defer_refresh(claim).await;
        }
        let errors = errors.into_iter().map(|error| turn.event(SessionEvent::Error { error }));
        let mut history = history;
        let fit = self.fit(&thread, &Log { read: &read, kept: &kept, rubrics: &rubrics }, &mut history).await;
        let start = turn.event(SpanEvent::ModelRequestStart {});
        let start_id = start.id.clone();
        let mut running = match idle {
            true => turn.running(),
            false => Vec::new(),
        };
        let ongoing =
            thread.grading.as_ref().map(|o| SpanEvent::OutcomeEvaluationOngoing { outcome_id: o.id.clone() });
        running.extend(ongoing.map(|span| Event { session_thread_id: None, ..turn.event(span) }));
        let mut opening =
            opening(taken, arrived, running, answers.into_iter().chain(errors).chain([start]).collect());
        if matches!(fit, Ok(Some(_))) {
            opening.engine.push((start_id.clone(), checkpoint::attempt()));
        }
        if !self.write(claim, &opening, Some(SessionStatus::Running)).await? {
            return Ok(());
        }
        session.status = SessionStatus::Running;
        match fit {
            Err(e) => return self.unfit(claim, &turn, session, &start_id, &e).await,
            Ok(Some(c)) => {
                let opened = Opened { turn: &turn, thread: &thread, start: &start_id };
                return self.compact(claim, session, opened, c, &servers.tools()).await;
            }
            Ok(None) => {}
        }
        let (tools, request) = self.request(&session, &thread, history.messages, &servers.tools());
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
                let mut done = batch(Vec::new(), Vec::new(), out.events);
                done.engine = out.engine;
                (done, out.end)
            }
        };
        self.commit(claim, end, &done, &session).await?;
        Ok(())
    }
}
