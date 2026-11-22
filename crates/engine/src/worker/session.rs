//! The session's own step: it takes the client's events, starts the threads
//! that have work, and makes the decisions that span the threads (the
//! interrupt, the grading round, the budget, idling the session). It runs one
//! at a time per session and calls no model, so the threads' steps, each on
//! its own row, run at the same time beside it.

use sessions::events::{Event, SessionEvent, StopReason};
use sessions::steps::{Claim, StepEnd};
use sessions::types::{Session, SessionStatus};
use shared::BackendError;

use super::Loop;
use super::settle::{quiet, round};
use super::status::busy;
use super::take::{Interrupt, Taken, cut_off};
use super::team::Thread;
use super::turn::{batch, opening};
use super::usage::{over_budget, so_far, tally};
use crate::turn::history::Kept;

impl Loop {
    pub(super) async fn session_step(
        &self,
        claim: &Claim,
        mut session: Session,
        log: &[Event],
        kept: &Kept,
        step: Taken,
    ) -> Result<(), BackendError> {
        let Taken { taken, arrived, answers, allowed, read, interrupt, due, all, .. } = step;
        let primary = Thread::primary(&session).turn();
        if interrupt == Interrupt::Second {
            self.stores
                .work
                .halt(&claim.workspace, claim.session.as_str())
                .await
                .map_err(BackendError::new)?;
        }
        // An interrupt stops every thread: each call it cut off is answered,
        // and the session idles.
        if interrupt != Interrupt::None {
            session.status = SessionStatus::Idle;
            let mut events: Vec<Event> = all.iter().flat_map(|(t, h)| cut_off(&t.turn(), h, log)).collect();
            events.extend(primary.idle(&session, StopReason::EndTurn {}));
            tally(&mut session, &read, kept);
            self.settled(claim, StepEnd::Done, &batch(taken, Vec::new(), events), &session, &[]).await?;
            return Ok(());
        }
        let spent = over_budget(&session, so_far(kept));
        let seen: Vec<Event> = read.iter().chain(&answers).cloned().collect();
        tally(&mut session, &seen, kept);
        let mut running = Vec::new();
        let mut rest = answers;
        let (wake, end) = match (spent, due.is_empty()) {
            (false, false) => {
                if session.status != SessionStatus::Running {
                    running.push(primary.event(SessionEvent::StatusRunning {}));
                    session.status = SessionStatus::Running;
                }
                let starting = all.iter().filter(|(t, _)| due.contains(&t.id) && !busy(&seen, &t.id));
                running.extend(starting.map(|(t, _)| {
                    let turn = t.turn();
                    turn.lifecycle(SessionEvent::ThreadStatusRunning { agent_name: turn.agent.clone() })
                }));
                (due, StepEnd::Done)
            }
            // The round's grader is a thread: the next step starts it.
            _ => match round(&session, &seen) {
                Some(start) => {
                    rest.push(start);
                    (Vec::new(), StepEnd::Again)
                }
                None => {
                    rest.extend(quiet(&mut session, &seen, spent).unwrap_or_default());
                    (Vec::new(), StepEnd::Done)
                }
            },
        };
        let end = match allowed {
            true => StepEnd::Requeue,
            false => end,
        };
        let write = opening(taken, arrived, running, rest);
        self.settled(claim, end, &write, &session, &wake).await?;
        Ok(())
    }
}
