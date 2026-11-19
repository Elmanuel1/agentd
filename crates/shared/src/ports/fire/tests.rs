use std::sync::Mutex;

use super::*;
use crate::ports::FireTarget;

/// Claims that answer `claim` to every take and count the finishes.
struct Claims {
    claim: Claim,
    finished: Mutex<usize>,
}

impl NativeFireClaims for Claims {
    async fn take(&self, _spec: &ScheduleSpec, _at: DateTime<Utc>) -> Result<Claim, FireError> {
        Ok(self.claim)
    }
    async fn enqueue(&self, _: &[Fire]) -> Result<(), FireError> {
        Ok(())
    }
    async fn claim_due(&self, _: usize) -> Result<Vec<Claimed>, FireError> {
        Ok(vec![])
    }
    async fn finish(&self, _: &Claimed, _: Outcome) -> Result<bool, FireError> {
        *self.finished.lock().unwrap() += 1;
        Ok(true)
    }
    async fn retry(&self, _: &Claimed, _: DateTime<Utc>) -> Result<bool, FireError> {
        Ok(true)
    }
}

fn claims(claim: Claim) -> Arc<Claims> {
    Arc::new(Claims { claim, finished: Mutex::new(0) })
}

fn registry() -> FireRegistry {
    FireRegistry::new(claims(Claim::Run { token: 1 }))
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
    registry.fire(&spec("deployment"), at).await.unwrap();
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
async fn a_fire_runs_and_finishes_only_when_its_claim_says_run() {
    for (claim, runs) in [(Claim::Run { token: 1 }, 1), (Claim::Done, 0), (Claim::Held, 0)] {
        let c = claims(claim);
        let registry = FireRegistry::new(c.clone());
        let handler = Arc::new(Recording::default());
        registry.register(handler.clone()).unwrap();
        assert_eq!(registry.fire(&spec("deployment"), Utc::now()).await.unwrap(), claim);
        assert_eq!(handler.fires.lock().unwrap().len(), runs, "{claim:?}");
        assert_eq!(*c.finished.lock().unwrap(), runs, "{claim:?}");
    }
}

#[tokio::test]
async fn a_failed_fire_is_not_finished() {
    let c = claims(Claim::Run { token: 1 });
    let registry = FireRegistry::new(c.clone());
    registry.register(Arc::new(Failing)).unwrap();
    assert!(registry.fire(&spec("deployment"), Utc::now()).await.is_err());
    assert_eq!(*c.finished.lock().unwrap(), 0);
}

#[tokio::test]
async fn fire_of_a_kind_with_no_handler_is_an_error() {
    let registry = registry();
    registry.register(Arc::new(Recording::default())).unwrap();
    let err = registry.fire(&spec("dream"), Utc::now()).await.unwrap_err();
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
