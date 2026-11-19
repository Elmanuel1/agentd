//! Scheduled fires through the [`FireRegistry`], over live Postgres: each
//! resource's handler runs the fire under the schedule's tenant, with no API
//! key involved, a fire that cannot run is an error, and a fire run again
//! makes no second run.
//!
//! Gated by `AGENTD_PG_TESTS=1` + `DATABASE_URL`; the dream test also needs
//! `AGENTD_AWS_ENDPOINT`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use deployments::DeploymentsService;
use std::sync::Arc;

use agents::Agent;
use async_trait::async_trait;
use blobs_s3::{BlobStore, S3BlobStore, S3Config};
use chrono::Utc;
use deployments::service::NativeDeploymentDeps;
use deployments::types::{Deployment, TriggerContext};
use dreams::types::{Candidate, Dream, DreamId, Verdict};
use dreams::{DreamsService, Proposer, ReviewGate};
use memory::MemoriesService;
use serde_json::json;
use shared::ports::fire::{FireError, FireRegistry};
use shared::ports::{FakeScheduler, FireTarget, ScheduleSpec, TenantRef};
use shared::{ApiError, AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgDeploymentsStore, PgDreamsStore, PgFireClaims, PgMemoryStore};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

fn tenant(tag: &str) -> AuthContext {
    let stamp = Timestamp::now().as_datetime().timestamp_micros();
    AuthContext {
        org: OrgId::new(format!("org_{tag}{stamp}")),
        workspace: WorkspaceId::new(format!("wrkspc_{tag}{stamp}")),
        api_key_id: format!("apikey_{tag}"),
    }
}

struct FakeDeps;

impl NativeDeploymentDeps for FakeDeps {
    async fn resolve_agent_ref(
        &self,
        _ctx: &AuthContext,
        _id: &str,
        _version: Option<u64>,
    ) -> Result<agents::AgentRef, ApiError> {
        let a: Agent =
            serde_json::from_str(include_str!("../../../../conformance/fixtures/agent.create.json")).unwrap();
        Ok(a.reference())
    }
    async fn check_environment(&self, _ctx: &AuthContext, _env: &str) -> Result<(), ApiError> {
        Ok(())
    }
    async fn create_session(
        &self,
        _ctx: &AuthContext,
        _d: &Deployment,
        _key: Option<&str>,
    ) -> Result<String, sessions::service::FireRefusal> {
        Ok("sesn_01fake".into())
    }
}

#[tokio::test]
async fn deployment_fire_runs_under_the_schedules_tenant() {
    if !enabled() {
        return;
    }
    let pg = backend().await;
    let svc =
        Arc::new(DeploymentsService::new(Arc::new(PgDeploymentsStore::new(pg.clone())), Arc::new(FakeDeps)));
    let fires = FireRegistry::new(Arc::new(PgFireClaims::new(pg)));
    for handler in deployments::module(svc.clone()).fire_handlers {
        fires.register(handler).unwrap();
    }
    let ctx = tenant("fire");
    let req = serde_json::from_value(json!({
        "name": "nightly",
        "agent": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
        "environment_id": "env_01ok",
        "initial_events": [{"type": "user.message", "content": [{"type": "text", "text": "run"}]}],
        "schedule": {"type": "cron", "expression": "0 3 * * *", "timezone": "UTC"}
    }))
    .unwrap();
    let d = svc.create(&ctx, req).await.unwrap();
    // The schedule the create registers through the outbox.
    let spec = ScheduleSpec {
        cron: d.schedule.expression.clone(),
        timezone: d.schedule.timezone.clone(),
        target: FireTarget::new("deployment", d.id.as_str()),
        tenant: TenantRef::of(&ctx),
    };

    let at = Utc::now();
    fires.run(&spec, at).await.unwrap();
    // A fire run again after a stop makes no second run.
    fires.run(&spec, at).await.unwrap();
    let scheduled_at = Timestamp::from_datetime(at);
    svc.fire_scheduled(&ctx, d.id.as_str(), scheduled_at).await.unwrap();

    let runs = svc.runs(&ctx, d.id.as_str(), None).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert!(matches!(runs[0].trigger_context, TriggerContext::Schedule { .. }));
    assert_eq!(runs[0].session_id.as_deref(), Some("sesn_01fake"));
}

struct NoCandidates;

#[async_trait]
impl Proposer for NoCandidates {
    async fn propose(&self, _ctx: &AuthContext, _dream: &Dream) -> Result<Vec<Candidate>, ApiError> {
        Ok(vec![])
    }
}

#[async_trait]
impl ReviewGate for NoCandidates {
    async fn review(&self, _: &AuthContext, _: &Candidate, _: Option<&str>) -> Result<Verdict, ApiError> {
        Err(ApiError::internal("no candidates to review"))
    }
}

#[tokio::test]
async fn dream_fire_runs_under_the_schedules_tenant_and_a_missing_dream_is_an_error() {
    if !enabled() {
        return;
    }
    let Some(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT").ok() else {
        eprintln!("skipping dream fire (needs AGENTD_AWS_ENDPOINT)");
        return;
    };
    let stamp = Timestamp::now().as_datetime().timestamp_micros();
    let blobs: Arc<dyn BlobStore> = Arc::new(
        S3BlobStore::connect(&S3Config {
            endpoint: Some(endpoint),
            path_style: true,
            create_bucket: true,
            ..S3Config::new(format!("agentdfire{stamp}"))
        })
        .await
        .unwrap(),
    );
    let scheduler = Arc::new(FakeScheduler::new());
    let pg = backend().await;
    let memories = Arc::new(MemoriesService::new(Arc::new(PgMemoryStore::new(pg.clone(), blobs))));
    let svc = Arc::new(DreamsService::new(
        Arc::new(PgDreamsStore::new(pg.clone())),
        memories.clone(),
        Arc::new(NoCandidates),
        Arc::new(NoCandidates),
        scheduler.clone(),
    ));
    let fires = FireRegistry::new(Arc::new(PgFireClaims::new(pg.clone())));
    for handler in dreams::module(svc.clone()).fire_handlers {
        fires.register(handler).unwrap();
    }
    let ctx = tenant("dream");
    let store = memories
        .create_store(&ctx, serde_json::from_value(json!({"name": "m", "description": ""})).unwrap())
        .await
        .unwrap();
    let req = serde_json::from_value(json!({
        "name": "nightly",
        "memory_store_id": store.id.as_str(),
        "schedule": {"type": "cron", "expression": "0 3 * * *", "timezone": "UTC"}
    }))
    .unwrap();
    let dream = svc.create(&ctx, req).await.unwrap();
    let spec = scheduler.registered().into_iter().find(|s| s.target.id == dream.id.as_str()).unwrap();

    fires.run(&spec, Utc::now()).await.unwrap();
    assert_eq!(svc.runs(&ctx, dream.id.as_str()).await.unwrap().len(), 1);

    let missing = ScheduleSpec { target: FireTarget::new("dream", DreamId::generate().as_str()), ..spec };
    let err = fires.run(&missing, Utc::now()).await.unwrap_err();
    assert!(matches!(&err, FireError::Failed(m) if m.contains("dream not found")), "{err}");
}
