use std::sync::{Arc, Mutex};

use agents::AgentRef;
use shared::{OrgId, Pagination, WorkspaceId};

use super::*;
use crate::service::NativeDeploymentDeps;
use crate::store::NativeDeploymentsStore;
use crate::types::{DeploymentRun, DeploymentStatus};

/// One deployment whose writes fail as stale `stale` times before one
/// applies; counts the reads and the writes.
struct Racing {
    stored: Mutex<Deployment>,
    stale: Mutex<usize>,
    reads: Mutex<usize>,
    writes: Mutex<Vec<(Deployment, Option<ScheduleChange>)>>,
}

impl Racing {
    fn new(stale: usize) -> Arc<Self> {
        Arc::new(Self {
            stored: Mutex::new(deployment()),
            stale: Mutex::new(stale),
            reads: Mutex::new(0),
            writes: Mutex::new(Vec::new()),
        })
    }
}

impl NativeDeploymentsStore for Racing {
    async fn insert(&self, _: &WorkspaceId, _: Deployment, _: &ScheduleChange) -> Result<(), StoreError> {
        Ok(())
    }
    async fn get(&self, _: &WorkspaceId, _: &DeploymentId) -> Result<Deployment, StoreError> {
        *self.reads.lock().unwrap() += 1;
        Ok(self.stored.lock().unwrap().clone())
    }
    async fn replace(
        &self,
        _: &WorkspaceId,
        d: Deployment,
        read: Timestamp,
        schedule: Option<&ScheduleChange>,
    ) -> Result<(), StoreError> {
        let mut stale = self.stale.lock().unwrap();
        if *stale > 0 {
            *stale -= 1;
            return Err(StoreError::Stale);
        }
        assert_eq!(read, self.stored.lock().unwrap().updated_at, "written against the last read");
        *self.stored.lock().unwrap() = d.clone();
        self.writes.lock().unwrap().push((d, schedule.cloned()));
        Ok(())
    }
    async fn list(&self, _: &WorkspaceId, _: &Pagination) -> Result<Vec<Deployment>, StoreError> {
        Ok(vec![])
    }
    async fn count(&self, _: &WorkspaceId) -> Result<usize, StoreError> {
        Ok(0)
    }
    async fn push_run(&self, _: &WorkspaceId, _: DeploymentRun) -> Result<(), StoreError> {
        Ok(())
    }
    async fn runs(
        &self,
        _: &WorkspaceId,
        _: &DeploymentId,
        _: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError> {
        Ok(vec![])
    }
}

struct NoDeps;

impl NativeDeploymentDeps for NoDeps {
    async fn resolve_agent_ref(
        &self,
        _: &AuthContext,
        _: &str,
        _: Option<u64>,
    ) -> Result<AgentRef, ApiError> {
        Err(ApiError::internal("unused"))
    }
    async fn check_environment(&self, _: &AuthContext, _: &str) -> Result<(), ApiError> {
        Ok(())
    }
    async fn create_session(
        &self,
        _: &AuthContext,
        _: &Deployment,
        _: Option<&str>,
    ) -> Result<String, sessions::service::FireRefusal> {
        Ok("sesn_01unused".into())
    }
}

fn deployment() -> Deployment {
    serde_json::from_value(serde_json::json!({
        "agent": {"id": "agent_011CZkYnKw7TfQv5X2mY8wP4", "type": "agent", "version": 1},
        "archived_at": null,
        "created_at": "2026-07-03T22:41:34.000000Z",
        "description": null,
        "environment_id": "env_011CZkYnKw7TfQv5X2mY8wP4",
        "id": "depl_011CZkYnKw7TfQv5X2mY8wP4",
        "initial_events": [],
        "metadata": {},
        "name": "d",
        "paused_reason": null,
        "resources": [],
        "schedule": {
            "expression": "0 0 1 1 *",
            "last_run_at": null,
            "timezone": "UTC",
            "type": "cron",
            "upcoming_runs_at": []
        },
        "status": "active",
        "type": "deployment",
        "updated_at": "2026-07-03T22:41:34.000000Z",
        "vault_ids": []
    }))
    .unwrap()
}

fn ctx() -> AuthContext {
    AuthContext {
        org: OrgId::new("org_r"),
        workspace: WorkspaceId::new("wrkspc_r"),
        api_key_id: "apikey_r".into(),
    }
}

#[tokio::test]
async fn rewrite_redoes_on_a_stale_write_and_gives_up_after_5() {
    let store = Racing::new(4);
    let svc = DeploymentsService::new(store.clone(), Arc::new(NoDeps));
    let paused = svc.pause(&ctx(), "depl_011CZkYnKw7TfQv5X2mY8wP4").await.unwrap();
    assert_eq!(paused.status, DeploymentStatus::Paused);
    assert_eq!(*store.reads.lock().unwrap(), 5, "read again after each stale write");
    let writes = store.writes.lock().unwrap().clone();
    assert_eq!(writes.len(), 1);
    assert_eq!(writes[0].0, paused);
    assert!(matches!(writes[0].1, Some(ScheduleChange::Pause { .. })));

    let store = Racing::new(5);
    let svc = DeploymentsService::new(store.clone(), Arc::new(NoDeps));
    let err = svc.pause(&ctx(), "depl_011CZkYnKw7TfQv5X2mY8wP4").await.unwrap_err();
    assert_eq!(err.status, 409);
    assert_eq!(*store.reads.lock().unwrap(), 5);
    assert!(store.writes.lock().unwrap().is_empty());
    assert_eq!(store.stored.lock().unwrap().status, DeploymentStatus::Active);
}

#[tokio::test]
async fn a_change_that_leaves_the_deployment_as_it_was_writes_nothing() {
    let store = Racing::new(0);
    let mut archived = deployment();
    archived.status = DeploymentStatus::Archived;
    archived.archived_at = Some(Timestamp::now());
    *store.stored.lock().unwrap() = archived.clone();
    let svc = DeploymentsService::new(store.clone(), Arc::new(NoDeps));
    assert_eq!(svc.archive(&ctx(), "depl_011CZkYnKw7TfQv5X2mY8wP4").await.unwrap(), archived);
    assert!(store.writes.lock().unwrap().is_empty());
}
