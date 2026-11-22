//! One step of a thread: read the session's log, call the model once for the
//! thread with no transaction open, and commit what the reply leads to. The
//! session's own step (`session.rs`) takes the client's events and starts the
//! threads, so a session's threads step at the same time.

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
use super::stall::abandoned;
use super::status::{arrived_for, busy};
use super::take::{Taken, take};
use super::turn::{batch, opening, thread_idle};
use super::usage::{over_budget, spent};
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
        let session = match self.stores.sessions.get(ws, id).await {
            Ok(session) => session,
            // Gone: its step goes too, should a commit have left one behind.
            Err(StoreError::NotFound) => {
                self.commit(claim, StepEnd::Done, &batch(Vec::new(), Vec::new(), Vec::new()), None).await?;
                return Ok(());
            }
            Err(e) => return Err(BackendError::new(e)),
        };
        let read = self.read(claim).await?;
        crate::metrics::read(read.len());
        let kept: Kept = read.iter().filter_map(|(e, k)| Some((e.id.clone(), k.clone()?))).collect();
        let log: Vec<Event> = read.into_iter().map(|(e, _)| e).collect();
        let queue = self.stores.work.queue_of(ws, id.as_str()).await.map_err(BackendError::new)?;
        let cloud = queue.is_none_or(|q| q == RUNTIME_QUEUE);
        let idle = session.status == SessionStatus::Idle;
        let rubrics = rubrics::read(&self.stores.files, ws, &log).await?;
        let step = take(&log, &session, cloud, &rubrics, &kept, claim.thread.as_ref());
        if claim.thread.is_none() {
            return self.session_step(claim, session, &log, step).await;
        }
        let mine = claim.thread.as_ref() == Some(&step.thread.id);
        if mine && step.history.unanswered.iter().any(Unanswered::for_loop) {
            return self.call_mcp(claim, session, step).await;
        }
        let Taken { taken, arrived, answers, thread, history, read, .. } = step;
        let turn = thread.turn();
        // Nothing for this thread now (its work is done, or it is gone, as a
        // grader after its round): the session's step decides what follows.
        if !mine || !history.awaits_model() || session.status != SessionStatus::Running {
            // A session step that read the log before this thread's last
            // commit may have started it again: it idles once more.
            let mut events = answers;
            if mine && busy(&read, &thread.id) {
                let event_ids: Vec<EventId> =
                    history.unanswered.iter().filter_map(|u| EventId::parse(&u.id).ok()).collect();
                let stop = match event_ids.is_empty() {
                    true => StopReason::EndTurn {},
                    false => StopReason::RequiresAction { event_ids },
                };
                events.push(turn.lifecycle(thread_idle(&turn, stop)));
            }
            self.commit(claim, StepEnd::Done, &batch(taken, Vec::new(), events), None).await?;
            return Ok(());
        }
        let before = self.calls(claim).await?.cost;
        if over_budget(&session, before) {
            let mut events = answers;
            events.push(turn.lifecycle(thread_idle(&turn, StopReason::BudgetReached {})));
            let failed = abandoned(
                &session,
                &thread,
                "The grader stopped on the session's budget before it gave a grade.",
            );
            events.extend(failed.as_ref().map(|(end, _)| end.clone()));
            let verdict = failed.map(|(_, eval)| eval);
            self.commit(claim, StepEnd::Done, &batch(taken, Vec::new(), events), verdict.as_ref()).await?;
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
        let mut running = Vec::new();
        let ongoing =
            thread.grading.as_ref().map(|o| SpanEvent::OutcomeEvaluationOngoing { outcome_id: o.id.clone() });
        running.extend(ongoing.map(|span| Event { session_thread_id: None, ..turn.event(span) }));
        let mut opening =
            opening(taken, arrived, running, answers.into_iter().chain(errors).chain([start]).collect());
        if matches!(fit, Ok(Some(_))) {
            opening.engine.push((start_id.clone(), checkpoint::attempt()));
        }
        if !self.write(claim, &opening, None).await? {
            return Ok(());
        }
        let opened = Opened { turn: &turn, thread: &thread, start: &start_id };
        match fit {
            Err(e) => return self.unfit(claim, opened, &session, &e).await,
            Ok(Some(c)) => return self.compact(claim, session, opened, c, &servers.tools()).await,
            Ok(None) => {}
        }
        let (tools, request) = self.request(ws, &session, &thread, history.messages, &servers.tools()).await;
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
        let mut verdict = None;
        let (done, end) = match answer {
            // As on live: the call's end, then the interrupt, then the idle.
            // The session's step takes the interrupt and idles the session.
            Err(_) => {
                let idle = turn.lifecycle(thread_idle(&turn, StopReason::EndTurn {}));
                (batch(Vec::new(), failed(None), vec![idle]), StepEnd::Done)
            }
            Ok(Err(e)) => {
                let mut events = failed(Some(&e));
                events.push(turn.lifecycle(thread_idle(&turn, StopReason::RetriesExhausted {})));
                let failed = abandoned(
                    &session,
                    &thread,
                    "The grader's model calls failed after every retry, before it gave a grade.",
                );
                events.extend(failed.as_ref().map(|(end, _)| end.clone()));
                verdict = failed.map(|(_, eval)| eval);
                (batch(Vec::new(), Vec::new(), events), StepEnd::Done)
            }
            Ok(Ok(reply)) => {
                let (usage, _) = spent(&reply);
                if let Err(e) = self.models.budget.spend(provider, usage.input_tokens).await {
                    tracing::warn!(session = %id, error = %e, "loop: could not count the call's tokens");
                }
                let later = self.stores.log.unprocessed_events(ws, id).await.map_err(BackendError::new)?;
                let more = arrived_for(&thread, &log, &later);
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
                settle(&mut out, &turn, &thread, &session, &read, &reply);
                let mut done = batch(Vec::new(), Vec::new(), out.events);
                done.engine = out.engine;
                verdict = out.evaluation;
                (done, out.end)
            }
        };
        self.commit(claim, end, &done, verdict.as_ref()).await?;
        Ok(())
    }
}
