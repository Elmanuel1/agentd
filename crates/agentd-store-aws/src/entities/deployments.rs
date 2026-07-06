//! `DeploymentsStore` on DynamoDB — the multi-replica exactly-once claim.
//!
//! Layout:
//! - `{prefix}_deployments`: `pk = workspace`, `sk = deployment_id`.
//! - `{prefix}_deployment_runs`: `pk = workspace#deployment`, `sk = created_at#run_id`.
//! - `{prefix}_deployment_claims`: `pk = workspace#deployment#scheduled_at`,
//!   `sk = "claim"` — a conditional `put_if_absent` is the atomic
//!   exactly-once boundary across replicas.

use agentd_core::{Pagination, WorkspaceId};
use agentd_deployments::store::{DeploymentsStore, StoreError};
use agentd_deployments::types::{Deployment, DeploymentId, DeploymentRun};
use async_trait::async_trait;

use crate::{AwsBackend, AwsError};

/// DynamoDB-backed deployments store.
#[derive(Debug, Clone)]
pub struct DynamoDeploymentsStore {
    aws: AwsBackend,
}

impl DynamoDeploymentsStore {
    /// Build; ensures all three tables exist.
    pub async fn new(aws: AwsBackend) -> Result<Self, AwsError> {
        aws.dynamo.ensure_table(&aws.table("deployments")).await?;
        aws.dynamo.ensure_table(&aws.table("deployment_runs")).await?;
        aws.dynamo.ensure_table(&aws.table("deployment_claims")).await?;
        Ok(Self { aws })
    }

    fn dtable(&self) -> String {
        self.aws.table("deployments")
    }
    fn rtable(&self) -> String {
        self.aws.table("deployment_runs")
    }
    fn ctable(&self) -> String {
        self.aws.table("deployment_claims")
    }
    fn run_pk(ws: &WorkspaceId, id: &DeploymentId) -> String {
        format!("{}#{}", ws.as_str(), id.as_str())
    }
}

fn be(e: AwsError) -> StoreError {
    StoreError::Backend(agentd_core::BackendError::new(e))
}

#[async_trait]
impl DeploymentsStore for DynamoDeploymentsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        self.aws
            .dynamo
            .put(&self.dtable(), ws.as_str(), d.id.as_str(), &d)
            .await
            .map_err(be)
    }

    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError> {
        self.aws
            .dynamo
            .get(&self.dtable(), ws.as_str(), id.as_str())
            .await
            .map_err(be)?
            .ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        // put is upsert; existence already implied by the caller having read it.
        self.aws
            .dynamo
            .put(&self.dtable(), ws.as_str(), d.id.as_str(), &d)
            .await
            .map_err(be)
    }

    async fn list(&self, ws: &WorkspaceId, page: &Pagination) -> Result<Vec<Deployment>, StoreError> {
        let all: Vec<Deployment> = self
            .aws
            .dynamo
            .query(&self.dtable(), ws.as_str(), None)
            .await
            .map_err(be)?;
        Ok(page.window(all))
    }

    async fn count(&self, ws: &WorkspaceId) -> Result<usize, StoreError> {
        let all: Vec<Deployment> = self
            .aws
            .dynamo
            .query(&self.dtable(), ws.as_str(), None)
            .await
            .map_err(be)?;
        Ok(all.len())
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DeploymentRun) -> Result<(), StoreError> {
        let pk = Self::run_pk(ws, &run.deployment_id);
        let sk = format!("{}#{}", run.created_at.to_wire(), run.id.as_str());
        self.aws
            .dynamo
            .put(&self.rtable(), &pk, &sk, &run)
            .await
            .map_err(be)
    }

    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError> {
        let pk = Self::run_pk(ws, deployment);
        let mut runs: Vec<DeploymentRun> =
            self.aws.dynamo.query(&self.rtable(), &pk, None).await.map_err(be)?;
        // Ascending by sk (created_at) → newest first.
        runs.reverse();
        Ok(runs
            .into_iter()
            .filter(|r| has_error.is_none_or(|want| r.error.is_some() == want))
            .collect())
    }

    async fn claim_fire(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        scheduled_at: &str,
    ) -> Result<bool, StoreError> {
        // Conditional write: the FIRST replica to write this key wins; every
        // other gets false. This is the real cross-replica exactly-once.
        let pk = format!("{}#{}#{scheduled_at}", ws.as_str(), deployment.as_str());
        self.aws
            .dynamo
            .put_if_absent(&self.ctable(), &pk, "claim")
            .await
            .map_err(be)
    }
}
