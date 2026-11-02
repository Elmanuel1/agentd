//! The loop of one session run: take in client events, run the threads that
//! have input, grade the outcome when the agent is done, and settle.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use sessions::events::{EventId, EventPayload, Rubric, SessionEvent, StopReason, UserEvent};
use sessions::types::{OutcomeEvaluation, OutcomeResult, Session, SessionStatus, ThreadId};
use shared::{ApiError, ContentBlock};
use tokio::sync::{watch, Notify};
use tokio::task::JoinSet;

use super::outcome::{self, Outcome};
use super::team::Team;
use super::SessionRun;
use crate::turn::history::outcome_prompt;
use crate::turn::{Delegation, Inbound, Runner, TurnEnd};

fn texts(content: &[ContentBlock]) -> String {
    content.iter().filter_map(ContentBlock::as_text).collect::<Vec<_>>().join("\n")
}

/// How often a run looks for client events no wake announced.
const POLL: std::time::Duration = std::time::Duration::from_secs(2);

impl SessionRun {
    pub(super) async fn drive(&self, session: Session, wake: Arc<Notify>) -> Result<(), ApiError> {
        let contract = self.engine.contract.clone();
        let events = contract.events(&self.ws, &self.session).await?;
        let mut rubrics = self.rubrics(&events).await?;
        let roster: Vec<_> = session
            .agent
            .config
            .multiagent
            .as_ref()
            .map(|m| m.roster_agents())
            .unwrap_or_default()
            .into_iter()
            .cloned()
            .collect();
        let mut agents = vec![&session.agent];
        agents.extend(roster.iter());
        let skills = self.prepare(&session, &agents).await?;
        let primary = self.primary(&session, &skills);
        let primary_id = primary.id.clone();
        let team = Arc::new(Team::new(contract.clone(), primary, roster, skills, wake.clone()));
        for e in &events {
            if let (EventPayload::Session(SessionEvent::ThreadCreated { agent_name }), Some(tid)) =
                (&e.payload, &e.session_thread_id)
            {
                team.restore(tid.clone(), agent_name);
            }
        }
        // Each thread looks once: one left mid-turn by a crash picks up again.
        for m in team.members() {
            team.mark(&m.ctx.id);
        }
        let rubric_text = |r: &Rubric, map: &HashMap<String, String>| match r {
            Rubric::Text { content } => content.clone(),
            Rubric::File { file_id } => map.get(file_id).cloned().unwrap_or_default(),
        };
        let mut outcome: Option<Outcome> = outcome::active(&events, |r| rubric_text(r, &rubrics));
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let mut runner = Runner {
            contract: contract.clone(),
            models: self.engine.models.clone(),
            tools: Arc::new(self.tools.clone()),
            delegation: Some(team.clone() as Arc<dyn Delegation>),
            rubrics: Arc::new(rubrics.clone()),
            cancel: cancel_rx,
            max_tokens: self.engine.max_tokens,
        };
        let mut set: JoinSet<(ThreadId, Result<TurnEnd, ApiError>)> = JoinSet::new();
        let mut running: HashSet<ThreadId> = HashSet::new();
        let mut blocked: HashMap<ThreadId, Vec<EventId>> = HashMap::new();
        let (mut active, mut failed, mut interrupted, mut grade_due) = (false, false, false, false);
        loop {
            let drained = contract.drain_user_events(&self.ws, &self.session).await?;
            interrupted |= Self::cancel_on(&drained, &cancel_tx);
            let mut answered = false;
            for e in &drained {
                let id = e.id.as_str().to_owned();
                match &e.payload {
                    EventPayload::User(UserEvent::Message { content }) => {
                        team.deliver(&primary_id, Some(Inbound { event: id, text: texts(content) }))
                    }
                    EventPayload::User(UserEvent::SystemMessage { content }) => {
                        team.deliver(
                            &primary_id,
                            Some(Inbound { event: id, text: format!("<system>{}</system>", texts(content)) }),
                        );
                    }
                    EventPayload::User(UserEvent::DefineOutcome {
                        description,
                        rubric,
                        max_iterations,
                        outcome_id: Some(oid),
                    }) => {
                        if let Rubric::File { file_id } = rubric {
                            if !rubrics.contains_key(file_id) {
                                let bytes = self.engine.assets.file(&self.ctx, file_id).await?;
                                rubrics.insert(file_id.clone(), String::from_utf8_lossy(&bytes).into_owned());
                                runner.rubrics = Arc::new(rubrics.clone());
                            }
                        }
                        let text = rubric_text(rubric, &rubrics);
                        outcome = Some(Outcome {
                            id: oid.clone(),
                            description: description.clone(),
                            rubric: text.clone(),
                            max_iterations: max_iterations.unwrap_or(outcome::DEFAULT_MAX_ITERATIONS),
                            iteration: 0,
                        });
                        team.deliver(
                            &primary_id,
                            Some(Inbound { event: id, text: outcome_prompt(description, &text) }),
                        );
                    }
                    EventPayload::User(
                        UserEvent::ToolConfirmation { .. }
                        | UserEvent::CustomToolResult { .. }
                        | UserEvent::ToolResult { .. },
                    ) => answered = true,
                    _ => {}
                }
            }
            if answered {
                // The thread the answer is for picks it up from the log.
                for (tid, _) in blocked.drain() {
                    team.mark(&tid);
                }
            }
            if interrupted {
                while set.join_next().await.is_some() {}
                if let Some(o) = outcome.take() {
                    let eval = OutcomeEvaluation {
                        outcome_id: o.id,
                        result: OutcomeResult::Interrupted,
                        explanation: None,
                        iteration: o.iteration,
                    };
                    contract.record_outcome(&self.ws, &self.session, eval).await?;
                }
                blocked.clear();
                break;
            }
            for tid in team.take_runnable() {
                let Some(member) = team.member(&tid) else { continue };
                if running.contains(&tid) {
                    continue;
                }
                if !active {
                    contract.set_status(&self.ws, &self.session, SessionStatus::Running, None).await?;
                    active = true;
                }
                blocked.remove(&tid);
                running.insert(tid.clone());
                team.set_busy(&tid, true);
                let r = runner.clone();
                set.spawn(async move { (tid, r.run(&member.ctx, &member.inbox).await) });
            }
            if set.is_empty() {
                let Some(o) = outcome.clone().filter(|_| grade_due && !failed && blocked.is_empty()) else {
                    break;
                };
                grade_due = false;
                match self.grade(&o).await? {
                    OutcomeResult::NeedsRevision => {
                        outcome = Some(Outcome { iteration: o.iteration + 1, ..o });
                        team.mark(&primary_id);
                    }
                    OutcomeResult::MaxIterationsReached => {
                        outcome = None;
                        team.mark(&primary_id);
                    }
                    _ => outcome = None,
                }
                continue;
            }
            tokio::select! {
                Some(joined) = set.join_next() => {
                    let (tid, end) = joined.map_err(|e| ApiError::internal(format!("thread task: {e}")))?;
                    running.remove(&tid);
                    team.set_busy(&tid, false);
                    match end? {
                        TurnEnd::Idle { .. } if tid == primary_id => grade_due = outcome.is_some(),
                        TurnEnd::Blocked(ids) => { blocked.insert(tid.clone(), ids); }
                        TurnEnd::Failed(why) if tid == primary_id => { failed = true; tracing::warn!(error = %why, "engine: primary thread failed"); }
                        TurnEnd::Failed(why) => {
                            if let Some(m) = team.member(&tid) {
                                team.reply(&m.ctx, &format!("The thread stopped: {why}")).await?;
                            }
                        }
                        _ => {}
                    }
                    if team.member(&tid).is_some_and(|m| !m.inbox.is_empty()) {
                        team.mark(&tid);
                    }
                }
                () = wake.notified() => {}
                // Client events sent to another replica wake no one here; look
                // for them now and then (an interrupt must not wait for a turn).
                () = tokio::time::sleep(POLL) => {}
            }
        }
        let stop = if failed {
            StopReason::RetriesExhausted {}
        } else if blocked.is_empty() {
            StopReason::EndTurn {}
        } else {
            StopReason::RequiresAction { event_ids: blocked.into_values().flatten().collect() }
        };
        if active || session.status == SessionStatus::Running || interrupted {
            self.settle(stop).await?;
        }
        Ok(())
    }
}
