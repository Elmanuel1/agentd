//! The fire path: a scheduled tick (`crate::fire::DeploymentFire`) and a
//! manual `/run`, each recorded as a run.

use chrono::Utc;
use shared::ports::ScheduleChange;
use shared::{ApiError, AuthContext, Timestamp};

use super::{map_store, parse_id, DeploymentsService};
use crate::fire::target;
use crate::run_error::{apply_run_error, classify_fire_error};
use crate::types::{
    Deployment, DeploymentRun, DeploymentRunId, DeploymentRunTag, DeploymentStatus, TriggerContext,
};

impl DeploymentsService {
    /// `POST /v1/deployments/{id}/run` — manual fire; allowed while paused.
    pub async fn run_manual(&self, ctx: &AuthContext, raw_id: &str) -> Result<DeploymentRun, ApiError> {
        let id = parse_id(raw_id)?;
        let d = self.store.get(&ctx.workspace, &id).await.map_err(map_store)?;
        if d.status == DeploymentStatus::Archived {
            return Err(ApiError::conflict_invalid_request("cannot run an archived deployment"));
        }
        self.fire(ctx, d, TriggerContext::Manual {}).await
    }

    /// The scheduled fire path: [`crate::fire::DeploymentFire`] and `/internal/fire`.
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
                let run = self.fire(ctx, d, TriggerContext::Schedule { scheduled_at }).await?;
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
        self.store.runs(&ctx.workspace, &id, has_error).await.map_err(map_store)
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
        self.store.push_run(&ctx.workspace, run.clone()).await.map_err(map_store)?;
        // Stamp last_run_at + refresh upcoming for scheduled fires.
        if let TriggerContext::Schedule { scheduled_at } = &trigger {
            d.schedule.last_run_at = Some(*scheduled_at);
            let _ = d.schedule.refresh_upcoming(Utc::now());
        }
        let archived = error.as_ref().is_some_and(|e| apply_run_error(&mut d, e));
        let change = archived.then(|| ScheduleChange::Delete { target: target(&d.id) });
        d.updated_at = Timestamp::now();
        self.store.replace(&ctx.workspace, d, change.as_ref()).await.map_err(map_store)?;
        Ok(run)
    }
}
