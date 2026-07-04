//! Deployment persistence — deployments + append-only run records.

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::types::{Deployment, DeploymentId, DeploymentRun};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such deployment.
    #[error("deployment not found")]
    NotFound,
}

/// Persistence port.
#[async_trait]
pub trait DeploymentsStore: Send + Sync {
    /// Insert.
    async fn insert(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError>;
    /// Fetch.
    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError>;
    /// Replace.
    async fn replace(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError>;
    /// List, insertion order.
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Deployment>, StoreError>;
    /// Count (for the ≤1000/org limit; keyed by workspace here, org check in api).
    async fn count(&self, ws: &WorkspaceId) -> Result<usize, StoreError>;
    /// Append a run record.
    async fn push_run(&self, ws: &WorkspaceId, run: DeploymentRun) -> Result<(), StoreError>;
    /// Runs for one deployment, newest first, optional error filter.
    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError>;

    /// Atomically claim one `(deployment, scheduled_at)` fire.
    ///
    /// Returns `true` if this call won the claim (the caller should fire),
    /// `false` if another replica already claimed it (skip). This is the
    /// exactly-once boundary across replicas — it MUST be atomic. The
    /// Postgres impl is `INSERT … ON CONFLICT DO NOTHING` on a
    /// `UNIQUE(deployment_id, scheduled_at)` index (rows-affected = the
    /// boolean). App code must never do a read-then-write dup check.
    async fn claim_fire(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        scheduled_at: &str,
    ) -> Result<bool, StoreError>;
}

type Entry = (Deployment, Vec<DeploymentRun>);

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemoryDeploymentsStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<DeploymentId, Entry>>>,
    /// Claimed fire keys (`ws/deployment/scheduled_at`) — the single-node
    /// stand-in for the Postgres unique index.
    claims: parking_lot::Mutex<std::collections::BTreeSet<String>>,
}

impl InMemoryDeploymentsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DeploymentsStore for InMemoryDeploymentsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(d.id.clone(), (d, Vec::new()));
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &DeploymentId) -> Result<Deployment, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(d, _)| d.clone())
            .ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Deployment) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&d.id))
            .ok_or(StoreError::NotFound)?;
        e.0 = d;
        Ok(())
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Deployment>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| m.values().map(|(d, _)| d.clone()).collect())
            .unwrap_or_default())
    }

    async fn count(&self, ws: &WorkspaceId) -> Result<usize, StoreError> {
        Ok(self.inner.read().get(ws).map(BTreeMap::len).unwrap_or(0))
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DeploymentRun) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&run.deployment_id))
            .ok_or(StoreError::NotFound)?;
        e.1.push(run);
        Ok(())
    }

    async fn runs(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        has_error: Option<bool>,
    ) -> Result<Vec<DeploymentRun>, StoreError> {
        let g = self.inner.read();
        let e = g
            .get(ws)
            .and_then(|m| m.get(deployment))
            .ok_or(StoreError::NotFound)?;
        let mut out: Vec<DeploymentRun> = e
            .1
            .iter()
            .filter(|r| has_error.is_none_or(|want| r.error.is_some() == want))
            .cloned()
            .collect();
        out.reverse(); // newest first
        Ok(out)
    }

    async fn claim_fire(
        &self,
        ws: &WorkspaceId,
        deployment: &DeploymentId,
        scheduled_at: &str,
    ) -> Result<bool, StoreError> {
        let key = format!("{}/{}/{scheduled_at}", ws.0, deployment.as_str());
        // insert() returns true iff the key was NOT already present — a
        // single atomic test-and-set, exactly like the DB unique index.
        Ok(self.claims.lock().insert(key))
    }
}
