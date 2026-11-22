//! How a thread's turn ends its step. A delegate's final text goes back to
//! its coordinator; a primary turn that ends on an outcome not yet met
//! starts a grading round; a turn that ends or waits while other threads
//! still work idles only its thread; otherwise the session idles.

use sessions::events::{Event, StopReason};
use sessions::steps::StepEnd;
use sessions::types::{Session, SessionStatus};

use super::grade::{active, begin};
use super::reply::Led;
use super::stall::{no_verdict, stalled};
use super::team::{Thread, busy, reply, threads, unfinished};
use super::turn::{Turn, thread_idle};
use super::usage::final_text;
use crate::model::ModelResponse;

/// End `out` as `turn`'s thread's turn ends.
pub(super) fn settle(
    out: &mut Led,
    turn: &Turn,
    thread: &Thread,
    session: &mut Session,
    log: &[Event],
    r: &ModelResponse,
) {
    let primary = Thread::primary(session).turn();
    let seen: Vec<Event> = log.iter().chain(&out.events).cloned().collect();
    let working: Vec<Thread> =
        threads(session, &seen).into_iter().filter(|t| t.id != turn.thread && busy(&seen, &t.id)).collect();
    let others = threads(session, &seen)
        .into_iter()
        .any(|t| !t.primary && t.id != turn.thread && unfinished(&seen, &t.id));
    let active = active(&seen);
    let grading = active.as_ref().is_some_and(|o| o.open.is_some());
    let outcome = active.filter(|o| o.open.is_none());
    match (out.idle.take(), thread.primary, thread.grading.is_some()) {
        // The grader ended a step without its grade: the next step asks for
        // it, until the grader has stalled and the round fails.
        (Some(StopReason::EndTurn {}), false, true) => {
            out.end = StepEnd::Again;
            let Some(o) = thread.grading.as_ref().filter(|_| stalled(&seen, &turn.thread)) else { return };
            let (end, eval) = no_verdict(&primary, o, r.usage.clone());
            out.events.push(end);
            out.evaluation = Some(eval);
        }
        (Some(stop @ StopReason::EndTurn {}), false, false) => {
            out.events.push(turn.lifecycle(thread_idle(turn, stop)));
            out.events.extend(reply(turn, &primary, &final_text(r)));
            out.end = StepEnd::Again;
        }
        // As on live: the round starts right after the reply that ends the
        // work, the thread still running; a reply while delegates still work
        // is not the end of it.
        (Some(StopReason::EndTurn {}), true, _) if outcome.is_some() && !others => {
            out.events.extend(outcome.map(|o| begin(&primary, &o)));
            out.end = StepEnd::Again;
        }
        // The session never idles with delegates working or a round open: the
        // grader runs, then the primary thread takes its verdict.
        (Some(stop @ StopReason::EndTurn {}), true, _) if others || grading => {
            out.events.push(turn.lifecycle(thread_idle(turn, stop)));
            out.end = StepEnd::Again;
        }
        // A thread that waits (on the worker, a confirmation or the client)
        // idles alone while another works, as on live: the session runs on
        // and the other threads take their steps.
        (Some(stop), _, _) if !working.is_empty() => {
            out.events.push(turn.lifecycle(thread_idle(turn, stop)));
            out.end = StepEnd::Again;
        }
        (Some(stop), _, _) => {
            session.status = SessionStatus::Idle;
            out.events.extend(turn.idle(session, stop));
        }
        (None, _, _) => {}
    }
}
