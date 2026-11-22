//! How a thread's turn ends its step, and the decisions the session's own
//! step makes for all of them. A thread's step ends only its own thread: a
//! delegate's final text goes back to its coordinator, a grader with no grade
//! is asked again. Idling the session and starting a grading round span the
//! threads, so only the session's step, which runs one at a time, makes them.

use sessions::events::{Event, EventPayload, SessionEvent, SpanEvent, StopReason, UserEvent};
use sessions::steps::StepEnd;
use sessions::types::{Session, SessionStatus};

use super::grade::{active, begin};
use super::reply::Led;
use super::stall::{no_verdict, stalled};
use super::status::{awaited, busy, stopped, unfinished};
use super::team::{Thread, reply, threads};
use super::turn::{Turn, thread_idle};
use super::usage::final_text;
use crate::model::ModelResponse;

/// End `out` as `turn`'s thread's turn ends.
pub(super) fn settle(
    out: &mut Led,
    turn: &Turn,
    thread: &Thread,
    session: &Session,
    log: &[Event],
    r: &ModelResponse,
) {
    let primary = Thread::primary(session).turn();
    let seen: Vec<Event> = log.iter().chain(&out.events).cloned().collect();
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
        }
        (Some(stop), _, _) => out.events.push(turn.lifecycle(thread_idle(turn, stop))),
        (None, _, _) => {}
    }
}

/// The round that starts once the primary thread has replied to the outcome
/// and no delegate is mid-turn, as on live: right after the reply that ends
/// the work. `None` while a round is open, a delegate works, or the primary
/// has not replied since the outcome was defined or last graded.
pub(super) fn round(session: &Session, log: &[Event]) -> Option<Event> {
    let outcome = active(log).filter(|o| o.open.is_none())?;
    let primary = Thread::primary(session);
    if threads(session, log).iter().any(|t| !t.primary && unfinished(log, &t.id)) {
        return None;
    }
    let replied = log.iter().rposition(|e| {
        e.session_thread_id.as_ref() == Some(&primary.id)
            && matches!(
                &e.payload,
                EventPayload::Session(SessionEvent::ThreadStatusIdle {
                    stop_reason: StopReason::EndTurn {},
                    ..
                })
            )
    })?;
    let asked = log.iter().rposition(|e| {
        matches!(
            &e.payload,
            EventPayload::User(UserEvent::DefineOutcome { .. })
                | EventPayload::Span(SpanEvent::OutcomeEvaluationEnd { .. })
        )
    })?;
    (replied > asked).then(|| begin(&primary.turn(), &outcome))
}

/// When no thread works and no round is open, the session idles, as on
/// live: on the budget or the retries a thread stopped on, on the calls its
/// threads wait for, else on the end of its turn. `None` while anything
/// works, or once the session is idle.
pub(super) fn quiet(session: &mut Session, log: &[Event]) -> Option<Vec<Event>> {
    let all = threads(session, log);
    let open = active(log).is_some_and(|o| o.open.is_some());
    if session.status != SessionStatus::Running || open || all.iter().any(|t| busy(log, &t.id)) {
        return None;
    }
    let event_ids: Vec<_> = all.iter().flat_map(|t| awaited(log, &t.id)).collect();
    let stopped_on = |r: fn(&StopReason) -> bool| all.iter().any(|t| stopped(log, &t.id).is_some_and(r));
    let spent = stopped_on(|r| matches!(r, StopReason::BudgetReached {}));
    let failed = stopped_on(|r| matches!(r, StopReason::RetriesExhausted {}));
    let stop = match (spent, failed, event_ids.is_empty()) {
        (true, _, _) => StopReason::BudgetReached {},
        (false, true, _) => StopReason::RetriesExhausted {},
        (false, false, true) => StopReason::EndTurn {},
        (false, false, false) => StopReason::RequiresAction { event_ids },
    };
    session.status = SessionStatus::Idle;
    let primary = Thread::primary(session).turn();
    Some(vec![
        primary.event(SessionEvent::Usage { usage: session.usage.clone(), budget: session.budget.clone() }),
        primary.event(SessionEvent::StatusIdle { stop_reason: stop }),
    ])
}
