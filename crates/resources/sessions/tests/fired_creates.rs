//! Two fired creates of one occurrence that overlap (DS-20): both ask for the
//! same session id, so the second insert fails on it. Each order of their
//! queue writes and inserts is scripted step by step.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::indexing_slicing
)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use sessions::events::{Event, EventId};
use sessions::store::{
    AnswerCheck, EventFilter, Head, NativeSessionEventLog, NativeSessionEventReads, NativeSessionReplace,
    NativeSessionsStore, SessionFilter, StoreError, Tail, Wake,
};
use sessions::types::{AgentRefInput, CreateSessionRequest};
use sessions::{NativeSessionDeps, NativeSessionWork, Session, SessionId, SessionsService};
use shared::limits::ModelCatalog;
use shared::{ApiError, AuthContext, OrgId, Pagination, Timestamp, WorkspaceId};

/// One create's step, named by the create's title.
type Step = (&'static str, &'static str);

/// The order the steps of both creates must take; each step waits its turn.
struct Script(Mutex<VecDeque<Step>>);

impl Script {
    async fn turn(&self, step: Step) {
        loop {
            {
                let mut steps = self.0.lock();
                if steps.front() == Some(&step) {
                    steps.pop_front();
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

fn who(s: &Session) -> &'static str {
    match s.title.as_deref() {
        Some("x") => "x",
        _ => "y",
    }
}

/// Session rows keyed by id, as the primary key keeps them.
struct Rows {
    script: Arc<Script>,
    rows: Mutex<BTreeMap<String, Session>>,
}

impl NativeSessionsStore for Rows {
    async fn insert(&self, _: &WorkspaceId, s: Session) -> Result<(), StoreError> {
        self.script.turn((who(&s), "insert")).await;
        let mut rows = self.rows.lock();
        if rows.contains_key(s.id.as_str()) {
            return Err(StoreError::Conflict);
        }
        rows.insert(s.id.as_str().to_owned(), s);
        Ok(())
    }
    async fn get(&self, _: &WorkspaceId, id: &SessionId) -> Result<Session, StoreError> {
        self.rows.lock().get(id.as_str()).cloned().ok_or(StoreError::NotFound)
    }
    async fn delete(&self, _: &WorkspaceId, _: &SessionId) -> Result<Session, StoreError> {
        unimplemented!()
    }
    async fn list(
        &self,
        _: &WorkspaceId,
        _: &SessionFilter,
        _: &Pagination,
    ) -> Result<Vec<Session>, StoreError> {
        unimplemented!()
    }
}

impl NativeSessionReplace for Rows {
    async fn replace(
        &self,
        _: &WorkspaceId,
        _: &Session,
        _: Session,
        _: &[webhooks::Delivery],
    ) -> Result<(), StoreError> {
        unimplemented!()
    }
}

/// The work queue as its upsert keeps it: the first queue write of a session
/// makes its item, a later one leaves it as it is.
struct Queue {
    script: Arc<Script>,
    item: Mutex<Option<(String, Option<String>, serde_json::Value)>>,
    seen: Mutex<BTreeMap<&'static str, serde_json::Value>>,
    ended: Mutex<usize>,
}

impl NativeSessionWork for Queue {
    async fn queue(
        &self,
        _: &AuthContext,
        s: &Session,
        environment: &serde_json::Value,
    ) -> Result<(), ApiError> {
        self.script.turn((who(s), "queue")).await;
        self.seen.lock().insert(who(s), environment.clone());
        let mut item = self.item.lock();
        if item.is_none() {
            *item = Some((s.id.as_str().to_owned(), s.title.clone(), environment.clone()));
        }
        Ok(())
    }
    async fn end(&self, _: &WorkspaceId, _: &SessionId) {
        *self.ended.lock() += 1;
    }
}

/// The agent and the environment; the environment's config type changes
/// between the two creates.
struct Deps {
    environments: Mutex<VecDeque<serde_json::Value>>,
}

impl NativeSessionDeps for Deps {
    async fn resolve_agent(
        &self,
        _: &AuthContext,
        _: &str,
        _: Option<u64>,
    ) -> Result<agents::AgentSnapshot, ApiError> {
        let agent: agents::Agent =
            serde_json::from_str(include_str!("../../../../conformance/fixtures/agent.create.json")).unwrap();
        Ok(agent.snapshot())
    }
    async fn resolve_environment(&self, _: &AuthContext, _: &str) -> Result<serde_json::Value, ApiError> {
        Ok(self.environments.lock().pop_front().unwrap())
    }
    async fn check_vaults(&self, _: &AuthContext, _: &[String]) -> Result<(), ApiError> {
        Ok(())
    }
}

/// An empty log: the creates send no initial events.
struct NoEvents;

impl NativeSessionEventReads for NoEvents {
    async fn events(&self, _: &WorkspaceId, _: &SessionId) -> Result<Vec<Event>, StoreError> {
        Ok(vec![])
    }
    async fn unprocessed_events(&self, _: &WorkspaceId, _: &SessionId) -> Result<Vec<Event>, StoreError> {
        Ok(vec![])
    }
    async fn events_page(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        _: &EventFilter,
        _: &Pagination,
    ) -> Result<Vec<Event>, StoreError> {
        Ok(vec![])
    }
    async fn tail(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        _: i64,
        _: &[i64],
        _: usize,
    ) -> Result<Option<Tail>, StoreError> {
        Ok(None)
    }
    async fn head(&self, _: &WorkspaceId, _: &SessionId) -> Result<Head, StoreError> {
        Ok(Head::default())
    }
}

impl NativeSessionEventLog for NoEvents {
    async fn append_events(
        &self,
        _: &WorkspaceId,
        _: &SessionId,
        _: &[Event],
        _: Wake,
    ) -> Result<(), StoreError> {
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
        _: Wake,
    ) -> Result<Result<(), ApiError>, StoreError> {
        unimplemented!()
    }
    async fn delete_events_of(&self, _: &[(WorkspaceId, SessionId)]) -> Result<(), StoreError> {
        unimplemented!()
    }
}

fn request(title: &str, id: &SessionId) -> CreateSessionRequest {
    CreateSessionRequest {
        agent: Some(AgentRefInput::Shorthand("agent_01XdReZgC7eHpg9b9Nvrwdcm".into())),
        environment_id: Some("env_01fired".into()),
        title: Some(title.into()),
        resources: vec![],
        vault_ids: vec![],
        metadata: Default::default(),
        budget: None,
        initial_events: vec![],
        deployment_id: Some("depl_01fired".into()),
        id: Some(id.clone()),
    }
}

#[tokio::test]
async fn two_overlapping_fired_creates_of_one_occurrence_make_one_session_and_keep_its_work_item_queued() {
    // x always inserts first; y queues first in the first order, second in the other.
    let orders: [[Step; 4]; 2] = [
        [("y", "queue"), ("x", "queue"), ("x", "insert"), ("y", "insert")],
        [("x", "queue"), ("y", "queue"), ("x", "insert"), ("y", "insert")],
    ];
    for order in orders {
        let script = Arc::new(Script(Mutex::new(order.into())));
        let rows = Arc::new(Rows { script: script.clone(), rows: Mutex::default() });
        let queue = Arc::new(Queue {
            script: script.clone(),
            item: Mutex::default(),
            seen: Mutex::default(),
            ended: Mutex::new(0),
        });
        let cloud = serde_json::json!({"config": {"type": "cloud"}});
        let self_hosted = serde_json::json!({"config": {"type": "self_hosted"}});
        let deps = Arc::new(Deps { environments: Mutex::new([cloud, self_hosted].into()) });
        let service = Arc::new(
            SessionsService::new(rows.clone(), Arc::new(NoEvents), deps, ModelCatalog::default_catalog())
                .with_work(queue.clone()),
        );
        let ctx = AuthContext {
            org: OrgId::new("org_f"),
            workspace: WorkspaceId::new("wrkspc_f"),
            api_key_id: "k".into(),
        };
        let id = SessionId::derived("depl_01fired@2026-11-19T04:00:00.000000Z");
        let (x, y) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                service.create_fired(&ctx, request("x", &id)),
                service.create_fired(&ctx, request("y", &id))
            )
        })
        .await
        .expect("both creates finish in their scripted order");
        let (x, y) = (x.unwrap(), y.unwrap());

        assert_eq!(x, y, "{order:?}: the losing create returns the session the first insert made");
        assert_eq!(x.title.as_deref(), Some("x"));
        assert_eq!(rows.rows.lock().len(), 1, "{order:?}: one session");
        assert_eq!(*queue.ended.lock(), 0, "{order:?}: the losing create does not stop the work item");
        let (session, title, environment) = queue.item.lock().clone().expect("a queued work item");
        assert_eq!(session, x.id.as_str(), "{order:?}: the work item is the winning session's");
        let first = order[0].0;
        assert_eq!(title.as_deref(), Some(first), "{order:?}: the first queue write made the item");
        let seen = queue.seen.lock().clone();
        assert_ne!(seen["x"], seen["y"], "the environment changed between the creates");
        assert_eq!(environment, seen[first], "{order:?}: its queue is the one the first queue write chose");
    }
}
