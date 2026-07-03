//! Environment persistence — **org-scoped** (shared across workspaces).

use std::collections::BTreeMap;

use agentd_core::OrgId;
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::types::{Environment, EnvironmentId};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such environment in this org.
    #[error("environment not found")]
    NotFound,
}

/// Persistence port for environments.
#[async_trait]
pub trait EnvironmentsStore: Send + Sync {
    /// Insert a new environment.
    async fn insert(&self, org: &OrgId, env: Environment) -> Result<(), StoreError>;
    /// Fetch one.
    async fn get(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError>;
    /// Replace one.
    async fn replace(&self, org: &OrgId, env: Environment) -> Result<(), StoreError>;
    /// Remove one.
    async fn delete(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError>;
    /// All environments in the org, insertion order.
    async fn list(&self, org: &OrgId) -> Result<Vec<Environment>, StoreError>;
}

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemoryEnvironmentsStore {
    inner: RwLock<BTreeMap<OrgId, BTreeMap<EnvironmentId, Environment>>>,
}

impl InMemoryEnvironmentsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EnvironmentsStore for InMemoryEnvironmentsStore {
    async fn insert(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(org.clone())
            .or_default()
            .insert(env.id.clone(), env);
        Ok(())
    }

    async fn get(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError> {
        self.inner
            .read()
            .get(org)
            .and_then(|m| m.get(id))
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn replace(&self, org: &OrgId, env: Environment) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let m = g.get_mut(org).ok_or(StoreError::NotFound)?;
        if !m.contains_key(&env.id) {
            return Err(StoreError::NotFound);
        }
        m.insert(env.id.clone(), env);
        Ok(())
    }

    async fn delete(&self, org: &OrgId, id: &EnvironmentId) -> Result<Environment, StoreError> {
        self.inner
            .write()
            .get_mut(org)
            .and_then(|m| m.remove(id))
            .ok_or(StoreError::NotFound)
    }

    async fn list(&self, org: &OrgId) -> Result<Vec<Environment>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(org)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default())
    }
}
