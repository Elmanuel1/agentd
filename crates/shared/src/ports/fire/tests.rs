use std::sync::Mutex;

use super::*;
use crate::ports::FireTarget;

/// A queue that records what it is given, or is `down`.
#[derive(Default)]
struct Queue {
    queued: Mutex<Vec<Fire>>,
    down: bool,
}

impl NativeFireClaims for Queue {
    async fn enqueue(&self, fires: &[Fire]) -> Result<(), FireError> {
        if self.down {
            return Err(FireError::Unavailable("database down".into()));
        }
        self.queued.lock().unwrap().extend_from_slice(fires);
        Ok(())
    }
    async fn claim_due(&self, _: usize) -> Result<Vec<Claimed>, FireError> {
        Ok(vec![])
    }
    async fn finish(&self, _: &Claimed, _: Outcome) -> Result<bool, FireError> {
        Ok(true)
    }
    async fn retry(&self, _: &Claimed, _: DateTime<Utc>) -> Result<bool, FireError> {
        Ok(true)
    }
}

fn registry() -> FireRegistry {
    FireRegistry::new(Arc::new(Queue::default()))
}

/// Fails every fire.
struct Failing;

#[async_trait]
impl FireHandler for Failing {
    fn kind(&self) -> &'static str {
        "deployment"
    }
    async fn fire(&self, _: &AuthContext, _: &str, _: DateTime<Utc>) -> Result<(), FireError> {
        Err(FireError::Failed("boom".into()))
    }
}

#[derive(Default)]
struct Recording {
    fires: Mutex<Vec<(AuthContext, String, DateTime<Utc>)>>,
    writers: Mutex<Vec<Writer>>,
}

#[async_trait]
impl FireHandler for Recording {
    fn kind(&self) -> &'static str {
        "deployment"
    }
    async fn fire(&self, tenant: &AuthContext, id: &str, at: DateTime<Utc>) -> Result<(), FireError> {
        self.fires.lock().unwrap().push((tenant.clone(), id.to_owned(), at));
        self.writers.lock().unwrap().push(Writer::current());
        Ok(())
    }
}

fn spec(kind: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: "0 0 * * *".into(),
        timezone: "UTC".into(),
        target: FireTarget::new(kind, "id_1"),
        tenant: TenantRef {
            org_id: "org_a".into(),
            workspace_id: "wrkspc_a".into(),
            api_key_id: "apikey_a".into(),
        },
    }
}

#[tokio::test]
async fn fire_runs_the_kinds_handler_under_the_schedules_tenant_as_its_key() {
    let registry = registry();
    let handler = Arc::new(Recording::default());
    registry.register(handler.clone()).unwrap();
    let at = Utc::now();
    registry.run(&spec("deployment"), at).await.unwrap();
    let fires = handler.fires.lock().unwrap();
    let (tenant, id, fired_at) = fires.first().unwrap();
    assert_eq!(tenant.org.as_str(), "org_a");
    assert_eq!(tenant.workspace.as_str(), "wrkspc_a");
    assert_eq!(tenant.api_key_id, "apikey_a");
    assert_eq!((id.as_str(), *fired_at), ("id_1", at));
    let writer = handler.writers.lock().unwrap().first().cloned().unwrap();
    assert_eq!(writer.actor, "apikey_a");
    assert!(writer.request_id.starts_with("req_"), "{}", writer.request_id);
}

#[tokio::test]
async fn an_enqueued_fire_is_queued_due_after_its_jitter_and_not_run() {
    let queue = Arc::new(Queue::default());
    let registry = FireRegistry::new(queue.clone());
    let handler = Arc::new(Recording::default());
    registry.register(handler.clone()).unwrap();
    let counted = Arc::new(Mutex::new(0));
    let into = counted.clone();
    registry.count_enqueued(move |n| *into.lock().unwrap() += n);
    let at = Utc::now();
    registry.enqueue(&spec("deployment"), at).await.unwrap();
    assert_eq!(*counted.lock().unwrap(), 1, "counted as enqueued");
    let queued = queue.queued.lock().unwrap().clone();
    let due_at = jitter::due_at(&spec("deployment"), at);
    assert_eq!(queued, vec![Fire { spec: spec("deployment"), scheduled_at: at, due_at }]);
    assert!(handler.fires.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_delivery_is_acknowledged_once_its_occurrence_is_queued() {
    let at = Utc::now();
    let queue = Arc::new(Queue::default());
    let registry = FireRegistry::new(queue.clone());
    assert!(deliver(&registry, Some((spec("deployment"), at)), "fires/0@1").await);
    assert_eq!(queue.queued.lock().unwrap().len(), 1);
    assert!(deliver(&registry, None, "fires/0@2").await, "a delivery with no fire is dropped");
    let down = FireRegistry::new(Arc::new(Queue { down: true, ..Queue::default() }));
    assert!(!deliver(&down, Some((spec("deployment"), at)), "fires/0@3").await, "delivered again");
}

#[tokio::test]
async fn a_failed_fire_is_an_error() {
    let registry = registry();
    registry.register(Arc::new(Failing)).unwrap();
    assert!(matches!(registry.run(&spec("deployment"), Utc::now()).await, Err(FireError::Failed(_))));
}

#[tokio::test]
async fn fire_of_a_kind_with_no_handler_is_an_error() {
    let registry = registry();
    registry.register(Arc::new(Recording::default())).unwrap();
    let err = registry.run(&spec("dream"), Utc::now()).await.unwrap_err();
    assert!(matches!(err, FireError::NoHandler(kind) if kind == "dream"));
}

#[test]
fn a_kind_registers_one_handler() {
    let registry = registry();
    registry.register(Arc::new(Recording::default())).unwrap();
    let err = registry.register(Arc::new(Recording::default())).unwrap_err();
    assert!(matches!(err, FireError::DuplicateKind(kind) if kind == "deployment"));
}

#[test]
fn fire_target_keys_and_json_are_stable() {
    let target = FireTarget::new("deployment", "depl_1");
    assert_eq!(target.key(), "deployment/depl_1");
    assert_eq!(
        serde_json::to_value(&target).unwrap(),
        serde_json::json!({"kind": "deployment", "id": "depl_1"})
    );
}

#[test]
fn tenant_round_trips_through_the_auth_context() {
    let ctx = AuthContext {
        org: OrgId::new("org_b"),
        workspace: WorkspaceId::new("wrkspc_b"),
        api_key_id: "apikey_b".into(),
    };
    assert_eq!(TenantRef::of(&ctx).auth_context(), ctx);
}
