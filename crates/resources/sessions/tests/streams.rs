//! A session's tail on one replica over a scripted log: what each read sends
//! to the session's streams, and when they end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::indexing_slicing
)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use sessions::events::{AgentEvent, Event, EventId, EventPayload, SessionEvent, UserEvent};
use sessions::signals::{LocalSignals, Signal, Signals};
use sessions::store::{
    AnswerCheck, DatabaseLoad, EventFilter, Head, SessionEventLog, SessionEventReads, SessionFilter,
    SessionsStore, StoreError, Tail,
};
use sessions::{SessionDeps, SessionId, SessionsService, Streams, Subscription};
use shared::limits::ModelCatalog;
use shared::{ApiError, AuthContext, BackendError, ErrorType, OrgId, Pagination, Timestamp, WorkspaceId};
use tokio::sync::broadcast::error::RecvError;

/// The log as the tail reads it: `head`, then each scripted read in turn,
/// then empty reads.
struct Scripted {
    head: Mutex<Result<Head, StoreError>>,
    reads: Mutex<VecDeque<Result<Option<Tail>, StoreError>>>,
    asked: Mutex<Vec<(i64, Vec<i64>)>>,
}

#[async_trait]
impl SessionEventReads for Scripted {
    async fn events(&self, _: &WorkspaceId, _: &SessionId) -> Result<Vec<Event>, StoreError> {
        unimplemented!()
    }
    async fn unprocessed_events(&self, _: &WorkspaceId, _: &SessionId) -> Result<Vec<Event>, StoreError> {
        unimplemented!()
    }
    async fn events_page(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        _: &EventFilter,
        _: &Pagination,
    ) -> Result<Vec<Event>, StoreError> {
        unimplemented!()
    }
    async fn tail(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        after: i64,
        pending: &[i64],
        _: usize,
    ) -> Result<Option<Tail>, StoreError> {
        self.asked.lock().push((after, pending.to_vec()));
        self.reads.lock().pop_front().unwrap_or(Ok(Some(Tail::default())))
    }
    async fn head(&self, _: &WorkspaceId, _: &SessionId) -> Result<Head, StoreError> {
        self.head.lock().clone()
    }
}

#[async_trait]
impl SessionEventLog for Scripted {
    async fn append_events(&self, _: &WorkspaceId, _: &SessionId, _: &[Event]) -> Result<(), StoreError> {
        unimplemented!()
    }
    async fn update_processed_at(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        _: &[EventId],
        _: Timestamp,
    ) -> Result<Vec<Event>, StoreError> {
        unimplemented!()
    }
    async fn append_answers(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        _: &[EventId],
        _: &[Event],
        _: &AnswerCheck<'_>,
    ) -> Result<Result<(), ApiError>, StoreError> {
        unimplemented!()
    }
    async fn delete_events_of(&self, _: &[(WorkspaceId, SessionId)]) -> Result<(), StoreError> {
        unimplemented!()
    }
}

/// One session, archived or not.
struct OneSession(sessions::Session);

#[async_trait]
impl SessionsStore for OneSession {
    async fn insert(&self, _: &WorkspaceId, _: sessions::Session) -> Result<(), StoreError> {
        unimplemented!()
    }
    async fn get(&self, _: &WorkspaceId, _: &SessionId) -> Result<sessions::Session, StoreError> {
        Ok(self.0.clone())
    }
    async fn replace(
        &self,
        _: &WorkspaceId,
        _: &sessions::Session,
        _: sessions::Session,
    ) -> Result<(), StoreError> {
        unimplemented!()
    }
    async fn delete(&self, _: &WorkspaceId, _: &SessionId) -> Result<sessions::Session, StoreError> {
        unimplemented!()
    }
    async fn list(
        &self,
        _: &WorkspaceId,
        _: &SessionFilter,
        _: &Pagination,
    ) -> Result<Vec<sessions::Session>, StoreError> {
        unimplemented!()
    }
}

struct NoDeps;

#[async_trait]
impl SessionDeps for NoDeps {
    async fn resolve_agent(
        &self,
        _: &AuthContext,
        _: &str,
        _: Option<u64>,
    ) -> Result<agents::AgentSnapshot, ApiError> {
        unimplemented!()
    }
    async fn resolve_environment(&self, _: &AuthContext, _: &str) -> Result<serde_json::Value, ApiError> {
        unimplemented!()
    }
}

struct Quiet;

impl DatabaseLoad for Quiet {
    fn overloaded(&self) -> bool {
        false
    }
}

struct Rig {
    log: Arc<Scripted>,
    signals: Arc<LocalSignals>,
    session: sessions::Session,
    ctx: AuthContext,
    service: SessionsService,
}

fn rig(head: Head, archived: bool, reads: Vec<Result<Option<Tail>, StoreError>>) -> Rig {
    let mut session: sessions::Session =
        serde_json::from_str(include_str!("../../../../conformance/fixtures/session.create.json")).unwrap();
    session.archived_at = archived.then(Timestamp::now);
    let log = Arc::new(Scripted {
        head: Mutex::new(Ok(head)),
        reads: Mutex::new(reads.into()),
        asked: Mutex::default(),
    });
    let signals = Arc::new(LocalSignals::default());
    let service = SessionsService::new(
        Arc::new(OneSession(session.clone())),
        log.clone(),
        Arc::new(NoDeps),
        ModelCatalog::default_catalog(),
    )
    .with_streams(Streams::new(signals.clone(), Arc::new(Quiet)));
    let ctx = AuthContext {
        org: OrgId::new("org_t"),
        workspace: WorkspaceId::new("wrkspc_t"),
        api_key_id: "k".into(),
    };
    Rig { log, signals, session, ctx, service }
}

impl Rig {
    async fn subscribe(&self) -> Subscription {
        self.service.subscribe(&self.ctx, self.session.id.as_str(), None).await.unwrap()
    }

    /// Signal the session, then let its tail read: signals that arrive
    /// before it reads coalesce into one read.
    async fn signal(&self) {
        let signal = Signal { workspace: self.ctx.workspace.clone(), session: self.session.id.clone() };
        self.signals.publish(&signal).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Inside the 5 s fallback timer, so only a signal explains the read.
const SOON: Duration = Duration::from_secs(2);

async fn next(sub: &mut Subscription) -> Result<Event, RecvError> {
    tokio::time::timeout(SOON, sub.recv()).await.expect("an event or the end in time")
}

fn message() -> Event {
    Event::new(EventPayload::User(UserEvent::Interrupt {}))
}

fn processed(e: &Event) -> Event {
    Event { processed_at: Some(Timestamp::now()), ..e.clone() }
}

fn read(archived: bool, events: Vec<(i64, Event)>) -> Result<Option<Tail>, StoreError> {
    Ok(Some(Tail { archived, events }))
}

#[tokio::test]
async fn a_client_event_is_sent_once_when_processed_and_never_queued() {
    let (queued, early) = (message(), processed(&message()));
    let done = processed(&queued);
    let reads = vec![
        read(false, vec![(1, queued.clone()), (2, early.clone())]),
        read(false, vec![(1, done.clone())]),
    ];
    let rig = rig(Head::default(), false, reads);
    let mut sub = rig.subscribe().await;
    rig.signal().await;
    assert_eq!(next(&mut sub).await.unwrap(), early, "one already processed is sent at once");
    rig.signal().await;
    assert_eq!(next(&mut sub).await.unwrap(), done, "the queued one only once processed");
    rig.signal().await;
    assert!(tokio::time::timeout(Duration::from_millis(300), sub.recv()).await.is_err(), "and never again");
    assert_eq!(rig.log.asked.lock()[..3], [(0, vec![]), (2, vec![1]), (2, vec![])]);
}

#[tokio::test]
async fn a_client_event_queued_before_the_stream_opened_is_sent_once_processed() {
    let e = message();
    let done = processed(&e);
    let head = Head { seq: 7, queued: vec![7] };
    let rig = rig(head, false, vec![read(false, vec![(7, e.clone())]), read(false, vec![(7, done.clone())])]);
    let mut sub = rig.subscribe().await;
    rig.signal().await;
    rig.signal().await;
    assert_eq!(next(&mut sub).await.unwrap(), done, "only its processed form");
    assert_eq!(rig.log.asked.lock()[..2], [(7, vec![7]), (7, vec![7])]);
}

#[tokio::test]
async fn archiving_ends_a_stream_opened_before_it_and_not_one_opened_after() {
    let rig_open = rig(Head::default(), false, vec![read(true, vec![])]);
    let mut sub = rig_open.subscribe().await;
    rig_open.signal().await;
    assert!(matches!(next(&mut sub).await, Err(RecvError::Closed)));

    let agent = Event::new(EventPayload::Agent(AgentEvent::Message { content: vec![] }));
    let rig_archived =
        rig(Head::default(), true, vec![read(true, vec![]), read(true, vec![(1, agent.clone())])]);
    let mut sub = rig_archived.subscribe().await;
    rig_archived.signal().await;
    rig_archived.signal().await;
    assert_eq!(next(&mut sub).await.unwrap(), agent, "still open after an archived read");
}

#[tokio::test]
async fn a_session_gone_ends_its_streams_with_session_deleted() {
    let rig = rig(Head::default(), false, vec![Ok(None)]);
    let mut sub = rig.subscribe().await;
    rig.signal().await;
    let last = next(&mut sub).await.unwrap();
    assert!(matches!(last.payload, EventPayload::Session(SessionEvent::Deleted {})));
    assert!(last.processed_at.is_some());
    assert!(matches!(next(&mut sub).await, Err(RecvError::Closed)));
}

#[tokio::test]
async fn a_failed_read_sends_nothing_and_the_next_signal_reads_again() {
    let e = processed(&message());
    let failed = Err(StoreError::Backend(BackendError::new(std::io::Error::other("down"))));
    let rig = rig(Head::default(), false, vec![failed, read(false, vec![(1, e.clone())])]);
    let mut sub = rig.subscribe().await;
    rig.signal().await;
    rig.signal().await;
    assert_eq!(next(&mut sub).await.unwrap(), e);
    assert_eq!(
        rig.log.asked.lock()[..2],
        [(0, vec![]), (0, vec![])],
        "the cursor did not move on the failure"
    );
}

#[tokio::test]
async fn missed_signals_wake_every_tail() {
    let e = processed(&message());
    let rig = rig(Head::default(), false, vec![read(false, vec![(1, e.clone())])]);
    let mut sub = rig.subscribe().await;
    let other = Signal { workspace: rig.ctx.workspace.clone(), session: SessionId::generate() };
    for _ in 0..sessions::signals::CAPACITY + 10 {
        rig.signals.publish(&other).await.unwrap();
    }
    assert_eq!(next(&mut sub).await.unwrap(), e, "the lagged listener woke this session's tail");
}

#[tokio::test]
async fn a_second_stream_of_a_session_shares_its_tail_until_both_leave() {
    let (one, two) = (processed(&message()), processed(&message()));
    let rig = rig(
        Head::default(),
        false,
        vec![read(false, vec![(1, one.clone())]), read(false, vec![(2, two.clone())])],
    );
    let (mut a, mut b) = (rig.subscribe().await, rig.subscribe().await);
    rig.signal().await;
    assert_eq!((next(&mut a).await.unwrap(), next(&mut b).await.unwrap()), (one.clone(), one));
    drop(a);
    rig.signal().await;
    assert_eq!(next(&mut b).await.unwrap(), two, "the tail stays while one stream is open");
    assert_eq!(rig.log.asked.lock().len(), 2, "one tail read the log, for both streams");
    drop(b);
    rig.signal().await;
    assert_eq!(rig.log.asked.lock().len(), 2, "the tail went with its last stream");
}

#[tokio::test]
async fn a_failed_head_read_refuses_the_stream_and_the_next_one_starts_its_own_tail() {
    let e = processed(&message());
    let rig = rig(Head::default(), false, vec![read(false, vec![(1, e.clone())])]);
    *rig.log.head.lock() = Err(StoreError::Backend(BackendError::new(std::io::Error::other("down"))));
    let refused = rig.service.subscribe(&rig.ctx, rig.session.id.as_str(), None).await;
    let err = refused.unwrap_err();
    let shown = (err.status, err.error_type, err.message.as_str(), err.extra.is_empty());
    assert_eq!(shown, (500, ErrorType::Api, "down", true));
    *rig.log.head.lock() = Ok(Head::default());
    let mut sub = rig.subscribe().await;
    rig.signal().await;
    assert_eq!(next(&mut sub).await.unwrap(), e);
}
