//! Deployment writes over live Postgres (DS-14): a write made after a read
//! never overwrites a change made since. Gated by `AGENTD_PG_TESTS=1` +
//! `DATABASE_URL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use agents::Agent;
use deployments::DeploymentsService;
use deployments::run_error::PausedReason;
use deployments::service::NativeDeploymentDeps;
use deployments::store::{DeploymentsStore, StoreError};
use deployments::types::{Deployment, DeploymentStatus, TriggerContext};
use serde_json::json;
use shared::{ApiError, AuthContext, OrgId, Timestamp, WorkspaceId};
use store_postgres::{PgBackend, PgDeploymentsStore};

fn enabled() -> bool {
    std::env::var("AGENTD_PG_TESTS").as_deref() == Ok("1")
}

async fn backend() -> PgBackend {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://agentd:agentd@localhost:5432/agentd".into());
    PgBackend::connect(&url).await.unwrap()
}

// Tests here run in parallel: the counter keeps two that start in the same microsecond in separate workspaces.
static SEQ: AtomicU64 = AtomicU64::new(0);

fn tenant() -> AuthContext {
    let stamp = format!(
        "{}_{}",
        Timestamp::now().as_datetime().timestamp_micros(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    );
    AuthContext {
        org: OrgId::new(format!("org_dw{stamp}")),
        workspace: WorkspaceId::new(format!("wrkspc_dw{stamp}")),
        api_key_id: "apikey_dw".into(),
    }
}

/// Creates each fire's session, pausing the deployment through the service
/// while it does: a pause between the fire's read and its write.
#[derive(Default)]
struct PausingDeps {
    service: OnceLock<Arc<DeploymentsService>>,
}

impl NativeDeploymentDeps for PausingDeps {
    async fn resolve_agent_ref(
        &self,
        _: &AuthContext,
        _: &str,
        _: Option<u64>,
    ) -> Result<agents::AgentRef, ApiError> {
        let a: Agent =
            serde_json::from_str(include_str!("../../../../conformance/fixtures/agent.create.json")).unwrap();
        Ok(a.reference())
    }
    async fn check_environment(&self, _: &AuthContext, _: &str) -> Result<(), ApiError> {
        Ok(())
    }
    async fn create_session(
        &self,
        ctx: &AuthContext,
        d: &Deployment,
        _: Option<&str>,
    ) -> Result<String, sessions::service::FireRefusal> {
        self.service.get().unwrap().pause(ctx, d.id.as_str()).await.unwrap();
        Ok("sesn_01paused".into())
    }
}

#[tokio::test]
async fn a_pause_between_a_fire_s_read_and_its_write_survives_the_fire() {
    if !enabled() {
        return;
    }
    let store = Arc::new(PgDeploymentsStore::new(backend().await));
    let deps = Arc::new(PausingDeps::default());
    let svc = Arc::new(DeploymentsService::new(store.clone(), deps.clone()));
    deps.service.set(svc.clone()).unwrap();
    let ctx = tenant();
    let req = serde_json::from_value(json!({
        "name": "nightly",
        "agent": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
        "environment_id": "env_01ok",
        "initial_events": [{"type": "user.message", "content": [{"type": "text", "text": "run"}]}],
        "schedule": {"type": "cron", "expression": "0 3 * * *", "timezone": "UTC"}
    }))
    .unwrap();
    let d = svc.create(&ctx, req).await.unwrap();
    let at = Timestamp::now();

    let run = svc.fire_scheduled(&ctx, d.id.as_str(), at).await.unwrap().expect("an active deployment fires");

    let after = svc.get(&ctx, d.id.as_str()).await.unwrap();
    assert_eq!(after.status, DeploymentStatus::Paused, "the pause survives the fire's write");
    assert_eq!(after.paused_reason, Some(PausedReason::Manual));
    assert_eq!(after.schedule.last_run_at, Some(at), "the fire's bookkeeping still lands");
    assert_eq!(run.session_id.as_deref(), Some("sesn_01paused"));
    assert!(matches!(run.trigger_context, TriggerContext::Schedule { .. }));
}

#[tokio::test]
async fn a_write_against_an_older_read_is_stale_and_changes_nothing() {
    if !enabled() {
        return;
    }
    let store = PgDeploymentsStore::new(backend().await);
    let deps = Arc::new(PausingDeps::default());
    let svc = DeploymentsService::new(Arc::new(store.clone()), deps);
    let ctx = tenant();
    let req = serde_json::from_value(json!({
        "name": "hourly",
        "agent": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
        "environment_id": "env_01ok",
        "initial_events": [{"type": "user.message", "content": [{"type": "text", "text": "run"}]}],
        "schedule": {"type": "cron", "expression": "0 * * * *", "timezone": "UTC"}
    }))
    .unwrap();
    let read = svc.create(&ctx, req).await.unwrap();
    let paused = svc.pause(&ctx, read.id.as_str()).await.unwrap();

    let mut renamed = read.clone();
    renamed.name = "renamed".into();
    let stale = DeploymentsStore::replace(&store, &ctx.workspace, renamed, read.updated_at, None).await;

    assert_eq!(stale, Err(StoreError::Stale));
    assert_eq!(svc.get(&ctx, read.id.as_str()).await.unwrap(), paused);
}

#[tokio::test]
async fn a_schedule_with_no_occurrence_in_the_next_year_is_refused() {
    if !enabled() {
        return;
    }
    let svc = DeploymentsService::new(
        Arc::new(PgDeploymentsStore::new(backend().await)),
        Arc::new(PausingDeps::default()),
    );
    let ctx = tenant();
    for cron in ["0 0 31 2 *", "0 0 30 2 *"] {
        let req = serde_json::from_value(json!({
            "name": "never",
            "agent": "agent_01XdReZgC7eHpg9b9Nvrwdcm",
            "environment_id": "env_01ok",
            "initial_events": [{"type": "user.message", "content": [{"type": "text", "text": "run"}]}],
            "schedule": {"type": "cron", "expression": cron, "timezone": "UTC"}
        }))
        .unwrap();
        let err = svc.create(&ctx, req).await.unwrap_err();
        assert_eq!(err.status, 400, "{cron}");
        assert_eq!(err.message, "`schedule.expression` does not produce any occurrences in the next year");
    }
    assert!(
        svc.list(&ctx, &shared::Pagination::from_params(&std::collections::HashMap::new()).unwrap())
            .await
            .unwrap()
            .is_empty(),
        "nothing was created"
    );
}
