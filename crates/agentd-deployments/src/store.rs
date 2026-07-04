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
}

type Entry = (Deployment, Vec<DeploymentRun>);

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemoryDeploymentsStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<DeploymentId, Entry>>>,
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
}
