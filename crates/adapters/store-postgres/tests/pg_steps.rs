//! The step queue over live Postgres: claims under a lease, takeovers,
//! wakes during a step, the per-workspace limit, and a session deleted
//! during its step. Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use sessions::events::{AgentEvent, Event, EventPayload, UserEvent};
use sessions::steps::{Claim, StepEnd, StepWrite, Steps};
use sessions::store::{SessionEventLog, SessionEventReads, SessionsStore, Wake};
use sessions::types::{Session, SessionId, SessionStatus};
use shared::{OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgSessionEventLog, PgSessionsStore, PgSteps, PgWorkStore};
use work::store::Reach;
use work::types::{Work, WorkData, WorkDataTag, WorkTag};
use work::{WorkItem, WorkReads, WorkState, WorkStore};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn uniq_ws(tag: &str) -> WorkspaceId {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    WorkspaceId::new(format!("wrkspc_{tag}{}{n}", Timestamp::now().as_datetime().timestamp_micros()))
}

/// The queue is shared, so these tests take turns.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Rig {
    _turn: tokio::sync::MutexGuard<'static, ()>,
    pg: PgBackend,
    sessions: PgSessionsStore,
    log: PgSessionEventLog,
    steps: Arc<PgSteps>,
}

async fn rig() -> Rig {
    let _turn = SERIAL.lock().await;
    let pg = backend().await;
    Rig {
        _turn,
        sessions: PgSessionsStore::new(pg.clone()),
        log: PgSessionEventLog::new(pg.clone()),
        steps: Arc::new(PgSteps::new(pg.clone())),
        pg,
    }
}

impl Rig {
    async fn session(&self, ws: &WorkspaceId) -> Session {
        let mut s: Session =
            serde_json::from_str(include_str!("../../../../conformance/fixtures/session.create.json"))
                .unwrap();
        s.id = SessionId::generate();
        self.sessions.insert(ws, s.clone()).await.unwrap();
        s
    }

    /// A client event that wakes the session's step.
    async fn wake(&self, ws: &WorkspaceId, id: &SessionId) -> Event {
        let e = Event::new(EventPayload::User(UserEvent::Interrupt {}));
        self.log.append_events(ws, id, std::slice::from_ref(&e), Wake::Step).await.unwrap();
        e
    }

    /// Claims until it takes `id`'s step; any other step goes back at once,
    /// and an empty or busy queue is tried a few more times.
    async fn claim_of(&self, owner: &str, lease: i64, id: &SessionId) -> Option<Claim> {
        let mut empty = 0;
        for _ in 0..500 {
            let Some(c) = self.steps.claim(owner, lease, 1000).await.unwrap() else {
                empty += 1;
                if empty > 5 {
                    return None;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                continue;
            };
            if c.session == *id {
                return Some(c);
            }
            self.steps.defer(&c, 0).await.unwrap();
        }
        None
    }
}

fn reply() -> Event {
    let mut e = Event::new(AgentEvent::Message { content: vec![] }.into());
    e.processed_at = Some(Timestamp::now());
    e
}

fn unchanged(_: &mut Session) -> bool {
    false
}

fn running(s: &mut Session) -> bool {
    s.status = SessionStatus::Running;
    true
}

#[tokio::test]
async fn a_client_event_queues_its_session_once_and_two_claimers_never_take_one_step() {
    // S-1, S-2.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("claim");
    let s = r.session(&ws).await;
    r.wake(&ws, &s.id).await;
    r.wake(&ws, &s.id).await;
    let claims: Vec<_> = (0..4)
        .map(|i| {
            let steps = r.steps.clone();
            tokio::spawn(async move { steps.claim(&format!("w{i}"), 60, 1000).await.unwrap() })
        })
        .collect();
    let mut ours = Vec::new();
    for c in claims {
        let Some(c) = c.await.unwrap() else { continue };
        // Another test's step goes back at once.
        if c.session != s.id {
            r.steps.defer(&c, 0).await.unwrap();
            continue;
        }
        ours.push(c);
    }
    assert!(ours.len() <= 1, "one step, one holder: {ours:?}");
    let held = match ours.pop() {
        Some(c) => c,
        None => r.claim_of("w9", 60, &s.id).await.unwrap(),
    };
    assert_eq!((held.attempts, held.wakes), (1, 1), "two wakes, one row");
    assert!(r.claim_of("w-check", 60, &s.id).await.is_none(), "a held step is not claimed again");
}

#[tokio::test]
async fn a_step_writes_and_commits_while_it_holds_its_claim() {
    // S-3: the read client events are stamped, the events appended and the
    // session row changed in the step's writes.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("commit");
    let s = r.session(&ws).await;
    let asked = r.wake(&ws, &s.id).await;
    let c = r.claim_of("w1", 60, &s.id).await.unwrap();
    let start = reply();
    let wrote = r
        .steps
        .write(
            &c,
            60,
            StepWrite {
                read: std::slice::from_ref(&asked.id),
                at: Timestamp::now(),
                events: std::slice::from_ref(&start),
                session: &running,
            },
        )
        .await
        .unwrap();
    assert!(wrote);
    assert_eq!(r.sessions.get(&ws, &s.id).await.unwrap().status, SessionStatus::Running);
    let last = reply();
    let done = r
        .steps
        .commit(
            &c,
            StepEnd::Done,
            StepWrite {
                read: &[],
                at: Timestamp::now(),
                events: std::slice::from_ref(&last),
                session: &unchanged,
            },
        )
        .await
        .unwrap();
    assert!(done);
    let events = r.log.events(&ws, &s.id).await.unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].processed_at.is_some(), "the read event is stamped");
    assert_eq!((events[1].id.clone(), events[2].id.clone()), (start.id, last.id));
    assert!(r.claim_of("w2", 60, &s.id).await.is_none(), "a finished step leaves the queue");
}

#[tokio::test]
async fn a_lease_that_runs_out_is_taken_over_and_the_late_commit_rolls_back() {
    // S-4, S-5.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("takeover");
    let s = r.session(&ws).await;
    r.wake(&ws, &s.id).await;
    let first = r.claim_of("slow", 1, &s.id).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
    let second = r.claim_of("fast", 60, &s.id).await.unwrap();
    assert_eq!(second.attempts, first.attempts + 1);
    let late = reply();
    let none: &[Event] = std::slice::from_ref(&late);
    assert!(!r
        .steps
        .write(&first, 60, StepWrite { read: &[], at: Timestamp::now(), events: none, session: &unchanged })
        .await
        .unwrap());
    assert!(!r
        .steps
        .commit(
            &first,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: none, session: &unchanged }
        )
        .await
        .unwrap());
    assert!(r
        .steps
        .commit(
            &second,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: &[reply()], session: &unchanged }
        )
        .await
        .unwrap());
    let events = r.log.events(&ws, &s.id).await.unwrap();
    assert_eq!(events.len(), 2, "the late commit appended nothing");
    assert!(events.iter().all(|e| e.id != late.id));
}

#[tokio::test]
async fn a_wake_during_a_step_runs_another_step_after_its_commit() {
    // S-6.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("wake");
    let s = r.session(&ws).await;
    r.wake(&ws, &s.id).await;
    let c = r.claim_of("w1", 60, &s.id).await.unwrap();
    r.wake(&ws, &s.id).await;
    assert!(r
        .steps
        .commit(
            &c,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: &[reply()], session: &unchanged }
        )
        .await
        .unwrap());
    let next = r.claim_of("w2", 60, &s.id).await.unwrap();
    assert_eq!(next.attempts, c.attempts + 1);
    assert!(r
        .steps
        .commit(
            &next,
            StepEnd::Again,
            StepWrite { read: &[], at: Timestamp::now(), events: &[], session: &unchanged }
        )
        .await
        .unwrap());
    assert!(r.claim_of("w3", 60, &s.id).await.is_some(), "a step that goes on is due again");
}

#[tokio::test]
async fn a_session_deleted_during_its_step_rolls_the_commit_back_and_runs_no_more_steps() {
    // E-4 with S1.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("delstep");
    let s = r.session(&ws).await;
    r.wake(&ws, &s.id).await;
    let c = r.claim_of("w1", 60, &s.id).await.unwrap();
    let before = r.log.head(&ws, &s.id).await.unwrap().seq;
    r.sessions.delete(&ws, &s.id).await.unwrap();
    assert!(!r
        .steps
        .commit(
            &c,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: &[reply()], session: &unchanged }
        )
        .await
        .unwrap());
    assert!(r.claim_of("w2", 60, &s.id).await.is_none());
    assert_eq!(r.log.head(&ws, &s.id).await.unwrap().seq, before, "nothing appended for a deleted session");
}

#[tokio::test]
async fn a_workspace_at_its_limit_waits_and_the_others_run() {
    // M-1: the claim's per-workspace count.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let (busy, other) = (uniq_ws("busy"), uniq_ws("other"));
    let (a, b, o) = (r.session(&busy).await, r.session(&busy).await, r.session(&other).await);
    for (ws, s) in [(&busy, &a), (&busy, &b), (&other, &o)] {
        r.wake(ws, &s.id).await;
    }
    let held = r.claim_of("w1", 60, &a.id).await.unwrap();
    let mut seen_b = false;
    let mut seen_o = false;
    // A claim that meets the busy workspace defers it and takes nothing.
    for _ in 0..200 {
        let Some(c) = r.steps.claim("w2", 60, 1).await.unwrap() else { continue };
        seen_b |= c.session == b.id;
        seen_o |= c.session == o.id;
        r.steps.defer(&c, 0).await.unwrap();
        if seen_o {
            break;
        }
    }
    assert!(!seen_b, "the busy workspace's next step waits");
    assert!(seen_o, "another workspace runs");
    r.steps
        .commit(
            &held,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: &[], session: &unchanged },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_wake_for_a_session_with_no_step_queues_none_when_it_only_wakes() {
    // P10: a running harness-path session's event wakes no step.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("ifq");
    let s = r.session(&ws).await;
    let e = Event::new(EventPayload::User(UserEvent::Interrupt {}));
    r.log.append_events(&ws, &s.id, std::slice::from_ref(&e), Wake::IfQueued).await.unwrap();
    assert!(r.claim_of("w1", 60, &s.id).await.is_none());
    r.wake(&ws, &s.id).await;
    let c = r.claim_of("w1", 60, &s.id).await.unwrap();
    r.log.append_events(&ws, &s.id, &[reply()], Wake::IfQueued).await.unwrap();
    r.steps
        .commit(
            &c,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: &[], session: &unchanged },
        )
        .await
        .unwrap();
    assert!(r.claim_of("w2", 60, &s.id).await.is_some(), "it woke the step the session had");
}

#[tokio::test]
async fn a_step_that_parks_a_built_in_call_queues_the_stopped_work_item_again() {
    // L-8.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let work = PgWorkStore::new(r.pg.clone());
    let ws = uniq_ws("requeue");
    let s = r.session(&ws).await;
    let now = Timestamp::now();
    let item = WorkItem {
        workspace: ws.clone(),
        org: OrgId::new("org_w"),
        queue: "env_steps".into(),
        work: Work {
            id: s.id.to_string(),
            acknowledged_at: None,
            actor: None,
            created_at: now,
            data: WorkData { id: s.id.to_string(), tag: WorkDataTag::Session },
            environment_id: "env_steps".into(),
            latest_heartbeat_at: None,
            metadata: BTreeMap::new(),
            secret: None,
            started_at: Some(now),
            state: WorkState::Queued,
            stop_requested_at: None,
            stopped_at: None,
            tag: WorkTag::Work,
        },
        last_heartbeat: None,
        lease_until: None,
        handed_out_at: None,
    };
    work.queue(&item).await.unwrap();
    work.stop(&ws, s.id.as_str(), &Reach::default(), true, now).await.unwrap();
    r.wake(&ws, &s.id).await;
    let c = r.claim_of("w1", 60, &s.id).await.unwrap();
    r.steps
        .commit(
            &c,
            StepEnd::Requeue,
            StepWrite { read: &[], at: Timestamp::now(), events: &[reply()], session: &unchanged },
        )
        .await
        .unwrap();
    let got = work.get(&ws, s.id.as_str(), &Reach::default()).await.unwrap().unwrap();
    assert_eq!(
        (got.work.state, got.queue.as_str()),
        (WorkState::Queued, "env_steps"),
        "{:?}",
        json!(got.work.state)
    );
}

#[tokio::test]
async fn the_counts_see_due_and_leased_steps() {
    // R-2.
    if !enabled() {
        return;
    }
    let r = rig().await;
    let ws = uniq_ws("counts");
    let (a, b) = (r.session(&ws).await, r.session(&ws).await);
    r.wake(&ws, &a.id).await;
    r.wake(&ws, &b.id).await;
    let before = r.steps.counts().await.unwrap();
    let c = r.claim_of("w1", 60, &a.id).await.unwrap();
    let after = r.steps.counts().await.unwrap();
    assert!(after.leased >= 1 && before.due >= 2, "{before:?} {after:?}");
    r.steps
        .commit(
            &c,
            StepEnd::Done,
            StepWrite { read: &[], at: Timestamp::now(), events: &[], session: &unchanged },
        )
        .await
        .unwrap();
}
