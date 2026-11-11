//! How a thread's turn ends its step. A delegate's final text goes back to
//! its coordinator; a primary turn that ends on an outcome not yet met
//! starts a grading round; a turn that ends while other threads still work
//! leaves the session running; otherwise the session idles.

use sessions::events::{Event, StopReason};
use sessions::steps::StepEnd;
use sessions::types::{Session, SessionStatus};

use super::grade::{active, begin};
use super::reply::Led;
use super::team::{busy, reply, threads, Thread};
use super::turn::{thread_idle, Turn};
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
    let others =
        threads(session, &seen).into_iter().any(|t| !t.primary && t.id != turn.thread && busy(&seen, &t.id));
    let outcome = active(&seen).filter(|o| o.open.is_none());
    match (out.idle.take(), thread.primary, thread.grading.is_some()) {
        // The grader ended a step without its verdict: the next step asks for it.
        (Some(StopReason::EndTurn {}), false, true) => out.end = StepEnd::Again,
        (Some(stop @ StopReason::EndTurn {}), false, false) => {
            out.events.push(turn.lifecycle(thread_idle(turn, stop)));
            out.events.extend(reply(turn, &primary, &final_text(r)));
            out.end = StepEnd::Again;
        }
        (Some(stop @ StopReason::EndTurn {}), true, _) if others || outcome.is_some() => {
            out.events.push(turn.lifecycle(thread_idle(turn, stop)));
            out.events.extend(outcome.map(|o| begin(&primary, &o)));
            out.end = StepEnd::Again;
        }
        (Some(stop), _, _) => {
            session.status = SessionStatus::Idle;
            out.events.extend(turn.idle(session, stop));
        }
        (None, _, _) => {}
    }
}
