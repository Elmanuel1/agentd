//! Deployment business logic: lifecycle, schedule registration, and the
//! fire path (`/internal/fire` + manual `/run`).

use std::sync::Arc;

use agentd_agents::AgentRef;
use agentd_core::limits::{required, validate_array_max, values};
use agentd_core::metadata::MetadataPolicy;
use agentd_core::ports::{FireTarget, ScheduleSpec, SchedulerPort};
use agentd_core::{ApiError, AuthContext, Pagination, Timestamp};
use async_trait::async_trait;
use chrono::Utc;

use crate::store::{DeploymentsStore, StoreError};
use crate::types::{
    CreateDeploymentRequest, Deployment, DeploymentId, DeploymentRun, DeploymentRunId,
    DeploymentRunTag, DeploymentStatus, DeploymentTag, PausedReason, RunError, RunErrorKind,
    TriggerContext, WireSchedule,
};

/// What deployments need from other entities — resolved via ports so tests
/// fake them and `agentd-api` wires the real services.
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
    /// Create a session for a fire; returns the session id.
    async fn create_session(
        &self,
        ctx: &AuthContext,
        deployment: &Deployment,
    ) -> Result<String, ApiError>;
}

/// Deployment service.
pub struct DeploymentsService {
    store: Arc<dyn DeploymentsStore>,
    deps: Arc<dyn DeploymentDeps>,
    scheduler: Arc<dyn SchedulerPort>,
}

impl std::fmt::Debug for DeploymentsService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentsService").finish_non_exhaustive()
    }
}

impl DeploymentsService {
    /// Build with store + deps + scheduler backend.
    pub fn new(
        store: Arc<dyn DeploymentsStore>,
        deps: Arc<dyn DeploymentDeps>,
        scheduler: Arc<dyn SchedulerPort>,
    ) -> Self {
        Self {
            store,
            deps,
            scheduler,
        }
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
            return Err(ApiError::invalid_request(
                "initial_events: Field required",
            ));
        }
        req.metadata.validate(MetadataPolicy::capped())?;
        let count = self
            .store
            .count(&ctx.workspace)
            .await
            .map_err(map_store)?;
        validate_array_max("deployments", count + 1, values::DEPLOYMENTS_PER_ORG_MAX)?;

        let agent = self
            .deps
            .resolve_agent_ref(ctx, agent_ref_input.id(), agent_ref_input.version())
            .await?;
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
        self.store
            .insert(&ctx.workspace, d.clone())
            .await
            .map_err(map_store)?;
        // Register the durable timer.
        self.scheduler
            .upsert(ScheduleSpec {
                cron: d.schedule.expression.clone(),
                timezone: d.schedule.timezone.clone(),
                target: FireTarget::Deployment {
                    id: d.id.as_str().to_owned(),
                },
                tenant: agentd_core::ports::TenantRef {
                    org_id: ctx.org.as_str().to_owned(),
                    workspace_id: ctx.workspace.as_str().to_owned(),
                },
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok(d)
    }

    /// `GET /v1/deployments/{id}`.
    pub async fn get(&self, ctx: &AuthContext, raw_id: &str) -> Result<Deployment, ApiError> {
        let id = parse_id(raw_id)?;
        self.store.get(&ctx.workspace, &id).await.map_err(map_store)
    }

    /// `GET /v1/deployments`.
    pub async fn list(
        &self,
        ctx: &AuthContext,
        page: &Pagination,
    ) -> Result<Vec<Deployment>, ApiError> {
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
        self.store
            .replace(&ctx.workspace, d.clone())
            .await
            .map_err(map_store)?;
        self.scheduler
            .pause(&FireTarget::Deployment {
                id: d.id.as_str().to_owned(),
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
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
        self.store
            .replace(&ctx.workspace, d.clone())
            .await
            .map_err(map_store)?;
        self.scheduler
            .unpause(&FireTarget::Deployment {
                id: d.id.as_str().to_owned(),
            })
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
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
            self.store
                .replace(&ctx.workspace, d.clone())
                .await
                .map_err(map_store)?;
            // Best-effort: the timer may already be gone.
            let _ = self
                .scheduler
                .delete(&FireTarget::Deployment {
                    id: d.id.as_str().to_owned(),
                })
                .await;
        }
        Ok(d)
    }

    /// `POST /v1/deployments/{id}/run` — manual fire; allowed while paused.
    pub async fn run_manual(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
    ) -> Result<DeploymentRun, ApiError> {
        let id = parse_id(raw_id)?;
        let d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if d.status == DeploymentStatus::Archived {
            return Err(ApiError::conflict_invalid_request(
                "cannot run an archived deployment",
            ));
        }
        self.fire(ctx, d, TriggerContext::Manual {}).await
    }

    /// The `/internal/fire` path — called by the scheduler backend.
    ///
    /// Contract: business failures are RECORDED (error run) and returned as
    /// success; the scheduler never retries. Paused/archived scheduled ticks
    /// are dropped silently (paused suppresses scheduled fires only).
    pub async fn fire_scheduled(
        &self,
        ctx: &AuthContext,
        raw_id: &str,
        scheduled_at: Timestamp,
    ) -> Result<Option<DeploymentRun>, ApiError> {
        let id = parse_id(raw_id)?;
        let d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        match d.status {
            DeploymentStatus::Paused | DeploymentStatus::Archived => Ok(None),
            DeploymentStatus::Active => {
                // Exactly-once across replicas: atomically CLAIM the
                // (deployment, scheduled_at) tick at the store. A read-then-
                // write dup check would let two replicas both pass and
                // double-fire; the claim is a single test-and-set (Postgres
                // UNIQUE index). Only the winner fires.
                let claimed = self
                    .store
                    .claim_fire(&ctx.workspace, &d.id, &scheduled_at.to_wire())
                    .await
                    .map_err(map_store)?;
                if !claimed {
                    return Ok(None);
                }
                let run = self
                    .fire(ctx, d, TriggerContext::Schedule { scheduled_at })
                    .await?;
                Ok(Some(run))
            }
        }
    }

    /// `GET /v1/deployment_runs?deployment_id=…&has_error=…`.
    pub async fn runs(
        &self,
        ctx: &AuthContext,
        raw_deployment_id: &str,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, ApiError> {
        let id = parse_id(raw_deployment_id)?;
        self.store
            .runs(&ctx.workspace, &id, has_error)
            .await
            .map_err(map_store)
    }

    /// One fire: freeze → create session → record run. Business failures
    /// become error runs; `agent_archived` additionally auto-archives.
    async fn fire(
        &self,
        ctx: &AuthContext,
        mut d: Deployment,
        trigger: TriggerContext,
    ) -> Result<DeploymentRun, ApiError> {
        let outcome = self.deps.create_session(ctx, &d).await;
        let (session_id, error) = match outcome {
            Ok(sid) => (Some(sid), None),
            Err(e) => (None, Some(classify_fire_error(&e))),
        };
        let run = DeploymentRun {
            agent: d.agent.clone(),
            created_at: Timestamp::now(),
            deployment_id: d.id.clone(),
            error: error.clone(),
            id: DeploymentRunId::generate(),
            session_id,
            trigger_context: trigger.clone(),
            tag: DeploymentRunTag::DeploymentRun,
        };
        self.store
            .push_run(&ctx.workspace, run.clone())
            .await
            .map_err(map_store)?;
        // Stamp last_run_at + refresh upcoming for scheduled fires.
        if let TriggerContext::Schedule { scheduled_at } = &trigger {
            d.schedule.last_run_at = Some(*scheduled_at);
            let _ = d.schedule.refresh_upcoming(Utc::now());
        }
        // Documented: agent archived/deleted → deployment auto-archives.
        if matches!(
            error,
            Some(RunError {
                kind: RunErrorKind::AgentArchived,
                ..
            })
        ) {
            d.status = DeploymentStatus::Archived;
            d.archived_at = Some(Timestamp::now());
            let _ = self
                .scheduler
                .delete(&FireTarget::Deployment {
                    id: d.id.as_str().to_owned(),
                })
                .await;
        }
        d.updated_at = Timestamp::now();
        self.store
            .replace(&ctx.workspace, d)
            .await
            .map_err(map_store)?;
        Ok(run)
    }
}

/// Map a session-create rejection into the fixed run-error taxonomy.
fn classify_fire_error(e: &ApiError) -> RunError {
    let msg = e.message.clone();
    let kind = if msg.contains("agent") && msg.contains("archived") {
        RunErrorKind::AgentArchived
    } else if msg.contains("environment") && msg.contains("archived") {
        RunErrorKind::EnvironmentArchived
    } else if msg.contains("vault") {
        RunErrorKind::VaultNotFound
    } else if e.status == 429 {
        RunErrorKind::SessionRateLimited
    } else {
        RunErrorKind::ServiceUnavailable
    };
    RunError { kind, message: msg }
}

fn guard_not_archived(d: &Deployment) -> Result<(), ApiError> {
    if d.status == DeploymentStatus::Archived {
        return Err(ApiError::conflict_invalid_request(
            "deployment is archived and immutable",
        ));
    }
    Ok(())
}

fn parse_id(raw: &str) -> Result<DeploymentId, ApiError> {
    DeploymentId::parse(raw).map_err(|e| ApiError::invalid_request(e.to_string()))
}

fn map_store(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("deployment not found"),
        StoreError::Backend(m) => ApiError::internal(m),
    }
}
