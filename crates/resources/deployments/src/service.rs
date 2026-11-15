//! Deployment business logic: lifecycle and schedule changes; the fire path
//! is in `service/firing.rs`.

use std::sync::Arc;

use agents::AgentRef;
use async_trait::async_trait;
use chrono::Utc;
use shared::limits::{required, validate_array_max, values};
use shared::metadata::MetadataPolicy;
use shared::ports::{ScheduleChange, ScheduleSpec, TenantRef};
use shared::{ApiError, AuthContext, Pagination, Timestamp};

use crate::fire::target;
use crate::run_error::PausedReason;
use crate::store::{DeploymentsStore, StoreError};
use crate::types::{
    CreateDeploymentRequest, Deployment, DeploymentId, DeploymentStatus, DeploymentTag, WireSchedule,
};

/// What deployments need from other entities — resolved via ports so tests
/// fake them and `api` wires the real services.
#[async_trait]
pub trait DeploymentDeps: Send + Sync {
    /// Resolve an agent id (+ pinned version) to a compact ref.
    /// Errors: not found / archived (→ `agent_archived` at fire time).
    async fn resolve_agent_ref(
        &self,
        ctx: &AuthContext,
        agent_id: &str,
        version: Option<u64>,
    ) -> Result<AgentRef, ApiError>;
    /// Verify the environment is usable (→ `environment_archived`).
    async fn check_environment(&self, ctx: &AuthContext, env_id: &str) -> Result<(), ApiError>;
    /// Create a session for a fire, with its initial events; returns the
    /// session id. Called again with the same `key`, it returns the session
    /// the first call made.
    async fn create_session(
        &self,
        ctx: &AuthContext,
        deployment: &Deployment,
        key: Option<&str>,
    ) -> Result<String, sessions::service::FireRefusal>;
}

/// Deployment service.
pub struct DeploymentsService {
    store: Arc<dyn DeploymentsStore>,
    deps: Arc<dyn DeploymentDeps>,
}

impl std::fmt::Debug for DeploymentsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentsService").finish_non_exhaustive()
    }
}

impl DeploymentsService {
    /// Build with store and deps; each deployment write carries its
    /// schedule's change, which the store's outbox relays to the scheduler.
    pub fn new(store: Arc<dyn DeploymentsStore>, deps: Arc<dyn DeploymentDeps>) -> Self {
        Self { store, deps }
    }

    /// `POST /v1/deployments`.
    pub async fn create(
        &self,
        ctx: &AuthContext,
        req: CreateDeploymentRequest,
    ) -> Result<Deployment, ApiError> {
        let name = required("name", req.name)?;
        let agent_ref_input = required("agent", req.agent)?;
        let environment_id = required("environment_id", req.environment_id)?;
        let schedule_input = required("schedule", req.schedule)?;
        if req.initial_events.is_empty() {
            return Err(ApiError::invalid_request("initial_events: Field required"));
        }
        req.metadata.validate(MetadataPolicy::capped())?;
        let count = self.store.count(&ctx.workspace).await.map_err(map_store)?;
        validate_array_max("deployments", count + 1, values::DEPLOYMENTS_PER_ORG_MAX)?;

        let agent = self.deps.resolve_agent_ref(ctx, agent_ref_input.id(), agent_ref_input.version()).await?;
        self.deps.check_environment(ctx, &environment_id).await?;

        let mut schedule = WireSchedule {
            expression: schedule_input.expression,
            last_run_at: None,
            timezone: schedule_input.timezone,
            kind: schedule_input.kind,
            upcoming_runs_at: vec![],
        };
        schedule.refresh_upcoming(Utc::now())?; // also validates cron+tz

        let now = Timestamp::now();
        let d = Deployment {
            agent,
            archived_at: None,
            created_at: now,
            description: req.description,
            environment_id,
            id: DeploymentId::generate(),
            initial_events: req.initial_events,
            metadata: req.metadata,
            name,
            paused_reason: None,
            resources: req.resources,
            schedule,
            status: DeploymentStatus::Active,
            tag: DeploymentTag::Deployment,
            updated_at: now,
            vault_ids: req.vault_ids,
        };
        let spec = ScheduleSpec {
            cron: d.schedule.expression.clone(),
            timezone: d.schedule.timezone.clone(),
            target: target(&d.id),
            tenant: TenantRef::of(ctx),
        };
        self.store
            .insert(&ctx.workspace, d.clone(), &ScheduleChange::Upsert { spec })
            .await
            .map_err(map_store)?;
        Ok(d)
    }

    /// `GET /v1/deployments/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Deployment, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/deployments`.
    pub async fn list(&self, ctx: &AuthContext, page: &Pagination) -> Result<Vec<Deployment>, ApiError> {
        self.store.list(&ctx.workspace, page).await.map_err(map_store)
    }

    /// `POST /v1/deployments/{id}/pause` — reversible; manual runs allowed.
    pub async fn pause(&self, ctx: &AuthContext, raw_id: &str) -> Result<Deployment, ApiError> {
        let id = parse_id(raw_id)?;
        let mut d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        guard_not_archived(&d)?;
        d.status = DeploymentStatus::Paused;
        d.paused_reason = Some(PausedReason::Manual);
        d.updated_at = Timestamp::now();
        let change = ScheduleChange::Pause { target: target(&d.id) };
        self.store.replace(&ctx.workspace, d.clone(), Some(&change)).await.map_err(map_store)?;
        Ok(d)
    }

    /// `POST /v1/deployments/{id}/unpause` — resumes next occurrence; no
    /// backfill.
    pub async fn unpause(&self, ctx: &AuthContext, raw_id: &str) -> Result<Deployment, ApiError> {
        let id = parse_id(raw_id)?;
        let mut d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        guard_not_archived(&d)?;
        d.status = DeploymentStatus::Active;
        d.paused_reason = None;
        d.updated_at = Timestamp::now();
        d.schedule.refresh_upcoming(Utc::now())?;
        let change = ScheduleChange::Unpause { target: target(&d.id) };
        self.store.replace(&ctx.workspace, d.clone(), Some(&change)).await.map_err(map_store)?;
        Ok(d)
    }

    /// `POST /v1/deployments/{id}/archive` — terminal; schedule deleted.
    pub async fn archive(&self, ctx: &AuthContext, raw_id: &str) -> Result<Deployment, ApiError> {
        let id = parse_id(raw_id)?;
        let mut d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if d.status != DeploymentStatus::Archived {
            d.status = DeploymentStatus::Archived;
            d.archived_at = Some(Timestamp::now());
            d.updated_at = Timestamp::now();
            let change = ScheduleChange::Delete { target: target(&d.id) };
            self.store.replace(&ctx.workspace, d.clone(), Some(&change)).await.map_err(map_store)?;
        }
        Ok(d)
    }
}

mod firing;

fn guard_not_archived(d: &Deployment) -> Result<(), ApiError> {
    if d.status == DeploymentStatus::Archived {
        return Err(ApiError::conflict_invalid_request("deployment is archived and immutable"));
    }
    Ok(())
}

pub(super) fn parse_id(raw: &str) -> Result<DeploymentId, ApiError> {
    DeploymentId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

pub(super) fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("deployment not found"),
        StoreError::Backend(m) => ApiError::internal(m.to_string()),
    }
}
