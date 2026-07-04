//! Deployment lifecycle + fire-contract tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use agentd_agents::AgentRef;
use agentd_core::ports::{FakeScheduler, FireTarget, SchedulerPort};
use agentd_core::{ApiError, AuthContext, OrgId, Timestamp, WorkspaceId};
use agentd_deployments::service::{DeploymentDeps, DeploymentsService};
use agentd_deployments::store::InMemoryDeploymentsStore;
use agentd_deployments::types::{
    CreateDeploymentRequest, Deployment, DeploymentStatus, RunErrorKind, TriggerContext,
};
use async_trait::async_trait;

/// Fake deps: session creation succeeds unless a failure mode is armed.
#[derive(Default)]
struct FakeDeps {
    fail_agent_archived: AtomicBool,
    fail_rate_limited: AtomicBool,
    sessions_created: AtomicUsize,
}

#[async_trait]
impl DeploymentDeps for FakeDeps {
    async fn resolve_agent_ref(
        &self,
        _ctx: &AuthContext,
        _agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentRef, ApiError> {
        let agent: agentd_agents::Agent =
            serde_json::from_str(include_str!("../../../fixtures/agent.create.json")).unwrap();
        let mut r = agent.reference();
        if let Some(v) = version {
            r.version = v;
        }
        Ok(r)
    }

    async fn check_environment(&self, _ctx: &AuthContext, _env: &str) -> Result<(), ApiError> {
        Ok(())
    }

    async fn create_session(
        &self,
        _ctx: &AuthContext,
        _d: &Deployment,
    ) -> Result<String, ApiError> {
        if self.fail_agent_archived.load(Ordering::SeqCst) {
            return Err(ApiError::conflict_invalid_request(
                "agent is archived; new sessions cannot reference it",
            ));
        }
        if self.fail_rate_limited.load(Ordering::SeqCst) {
            return Err(ApiError::rate_limited("Too many requests"));
        }
        self.sessions_created.fetch_add(1, Ordering::SeqCst);
        Ok(format!(
            "sesn_01Fake{:018}",
            self.sessions_created.load(Ordering::SeqCst)
        ))
    }
}

struct Rig {
    svc: DeploymentsService,
    deps: Arc<FakeDeps>,
    scheduler: Arc<FakeScheduler>,
}

fn rig() -> Rig {
    let deps = Arc::new(FakeDeps::default());
    let scheduler = Arc::new(FakeScheduler::new());
    let svc = DeploymentsService::new(
        Arc::new(InMemoryDeploymentsStore::new()),
        deps.clone(),
        scheduler.clone(),
    );
    Rig {
        svc,
        deps,
        scheduler,
    }
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId("org_t".into()),
        workspace: WorkspaceId("wrkspc_01t".into()),
        api_key_id: "apikey_01t".into(),
    }
}

fn create_req() -> CreateDeploymentRequest {
    serde_json::from_value(serde_json::json!({
        "name": "probe-depl",
        "agent": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
        "environment_id": "env_01ok",
        "initial_events": [
            {"type":"user.message","content":[{"type":"text","text":"run"}]}
        ],
        "schedule": {"type":"cron","expression":"0 0 1 1 *","timezone":"America/New_York"}
    }))
    .unwrap()
}

#[tokio::test]
async fn create_matches_live_fixture_shape() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    let v = serde_json::to_value(&d).unwrap();
    assert_eq!(v["type"], "deployment");
    assert_eq!(v["status"], "active");
    assert_eq!(v["paused_reason"], serde_json::Value::Null);
    // Compact agent ref, not a snapshot.
    assert_eq!(v["agent"]["type"], "agent");
    assert!(v["agent"].get("system").is_none());
    // 5 upcoming runs, tz-correct.
    let runs = v["schedule"]["upcoming_runs_at"].as_array().unwrap();
    assert_eq!(runs.len(), 5);
    assert!(runs[0].as_str().unwrap().ends_with("T05:00:00.000000Z"));
    assert!(d.id.as_str().starts_with("depl_01"));
    // Timer registered.
    assert_eq!(r.scheduler.registered().len(), 1);
}

#[tokio::test]
async fn pause_unpause_archive_lifecycle_matches_live() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    // Live probe: pause → status=paused, paused_reason={type:manual}
    let paused = r.svc.pause(&ctx(), d.id.as_str()).await.unwrap();
    assert_eq!(paused.status, DeploymentStatus::Paused);
    assert_eq!(
        serde_json::to_value(&paused).unwrap()["paused_reason"],
        serde_json::json!({"type":"manual"})
    );
    let target = FireTarget::Deployment {
        id: d.id.as_str().to_owned(),
    };
    assert!(r.scheduler.describe(&target).await.unwrap().paused);

    let active = r.svc.unpause(&ctx(), d.id.as_str()).await.unwrap();
    assert_eq!(active.status, DeploymentStatus::Active);
    assert!(!r.scheduler.describe(&target).await.unwrap().paused);

    let archived = r.svc.archive(&ctx(), d.id.as_str()).await.unwrap();
    assert_eq!(archived.status, DeploymentStatus::Archived);
    // Timer deleted; archive is terminal — pause now conflicts.
    assert!(r.scheduler.describe(&target).await.is_err());
    assert_eq!(
        r.svc.pause(&ctx(), d.id.as_str()).await.unwrap_err().status,
        409
    );
}

#[tokio::test]
async fn manual_run_works_even_while_paused() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    r.svc.pause(&ctx(), d.id.as_str()).await.unwrap();
    let run = r.svc.run_manual(&ctx(), d.id.as_str()).await.unwrap();
    assert!(matches!(run.trigger_context, TriggerContext::Manual {}));
    assert!(run.session_id.is_some());
    assert!(run.error.is_none());
    assert!(run.id.as_str().starts_with("drun_01"));
}

#[tokio::test]
async fn scheduled_fire_dropped_while_paused_no_backfill() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    r.svc.pause(&ctx(), d.id.as_str()).await.unwrap();
    let out = r
        .svc
        .fire_scheduled(&ctx(), d.id.as_str(), Timestamp::now())
        .await
        .unwrap();
    assert!(out.is_none(), "paused scheduled tick is dropped");
    assert!(r.svc.runs(&ctx(), d.id.as_str(), None).await.unwrap().is_empty());
}

#[tokio::test]
async fn scheduled_fire_is_idempotent_per_scheduled_at() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    let tick = Timestamp::now();
    let first = r
        .svc
        .fire_scheduled(&ctx(), d.id.as_str(), tick)
        .await
        .unwrap();
    assert!(first.is_some());
    // Same (deployment, scheduled_at) again → dropped, no duplicate run.
    let second = r
        .svc
        .fire_scheduled(&ctx(), d.id.as_str(), tick)
        .await
        .unwrap();
    assert!(second.is_none());
    assert_eq!(r.svc.runs(&ctx(), d.id.as_str(), None).await.unwrap().len(), 1);
    assert_eq!(r.deps.sessions_created.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rate_limited_fire_records_error_run_no_retry() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    r.deps.fail_rate_limited.store(true, Ordering::SeqCst);
    let run = r
        .svc
        .fire_scheduled(&ctx(), d.id.as_str(), Timestamp::now())
        .await
        .unwrap()
        .unwrap();
    assert!(run.session_id.is_none());
    assert_eq!(run.error.as_ref().unwrap().kind, RunErrorKind::SessionRateLimited);
    // has_error filter works.
    let errored = r.svc.runs(&ctx(), d.id.as_str(), Some(true)).await.unwrap();
    assert_eq!(errored.len(), 1);
    // Deployment still active — the next occurrence just tries again.
    assert_eq!(
        r.svc.get(&ctx(), d.id.as_str()).await.unwrap().status,
        DeploymentStatus::Active
    );
}

#[tokio::test]
async fn agent_archived_fire_auto_archives_deployment() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    r.deps.fail_agent_archived.store(true, Ordering::SeqCst);
    let run = r
        .svc
        .fire_scheduled(&ctx(), d.id.as_str(), Timestamp::now())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.error.as_ref().unwrap().kind, RunErrorKind::AgentArchived);
    // Documented: agent archived → deployment AUTO-ARCHIVED.
    assert_eq!(
        r.svc.get(&ctx(), d.id.as_str()).await.unwrap().status,
        DeploymentStatus::Archived
    );
}

#[tokio::test]
async fn scheduled_fire_stamps_last_run_at() {
    let r = rig();
    let d = r.svc.create(&ctx(), create_req()).await.unwrap();
    let tick = Timestamp::now();
    r.svc
        .fire_scheduled(&ctx(), d.id.as_str(), tick)
        .await
        .unwrap();
    let got = r.svc.get(&ctx(), d.id.as_str()).await.unwrap();
    assert_eq!(got.schedule.last_run_at, Some(tick));
}

#[tokio::test]
async fn required_fields_and_bad_schedule() {
    let r = rig();
    let mut req = create_req();
    req.schedule = None;
    assert_eq!(
        r.svc.create(&ctx(), req).await.unwrap_err().message,
        "schedule: Field required"
    );
    let mut req = create_req();
    req.initial_events = vec![];
    assert_eq!(
        r.svc.create(&ctx(), req).await.unwrap_err().message,
        "initial_events: Field required"
    );
    let req: CreateDeploymentRequest = serde_json::from_value(serde_json::json!({
        "name":"x","agent":"agent_01XdReZgC7eHpg9b9Nvrwdcm","environment_id":"env_01ok",
        "initial_events":[{"type":"user.message","content":[{"type":"text","text":"go"}]}],
        "schedule":{"type":"cron","expression":"99 * * * *","timezone":"UTC"}
    }))
    .unwrap();
    let e = r.svc.create(&ctx(), req).await.unwrap_err();
    assert!(e.message.starts_with("schedule:"));
}
