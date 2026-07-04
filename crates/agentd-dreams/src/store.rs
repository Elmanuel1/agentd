//! Dream persistence.

use std::collections::BTreeMap;

use agentd_core::WorkspaceId;
use async_trait::async_trait;
use parking_lot::RwLock;

use crate::types::{Dream, DreamId, DreamRun};

/// Store-level errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// No such dream.
    #[error("dream not found")]
    NotFound,
}

/// Persistence port.
#[async_trait]
pub trait DreamsStore: Send + Sync {
    /// Insert.
    async fn insert(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError>;
    /// Fetch.
    async fn get(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Dream, StoreError>;
    /// Replace.
    async fn replace(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError>;
    /// List.
    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Dream>, StoreError>;
    /// Append a run.
    async fn push_run(&self, ws: &WorkspaceId, run: DreamRun) -> Result<(), StoreError>;
    /// Runs newest first.
    async fn runs(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Vec<DreamRun>, StoreError>;
}

type Entry = (Dream, Vec<DreamRun>);

/// In-memory store.
#[derive(Debug, Default)]
pub struct InMemoryDreamsStore {
    inner: RwLock<BTreeMap<WorkspaceId, BTreeMap<DreamId, Entry>>>,
}

impl InMemoryDreamsStore {
    /// New empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DreamsStore for InMemoryDreamsStore {
    async fn insert(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError> {
        self.inner
            .write()
            .entry(ws.clone())
            .or_default()
            .insert(d.id.clone(), (d, Vec::new()));
        Ok(())
    }

    async fn get(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Dream, StoreError> {
        self.inner
            .read()
            .get(ws)
            .and_then(|m| m.get(id))
            .map(|(d, _)| d.clone())
            .ok_or(StoreError::NotFound)
    }

    async fn replace(&self, ws: &WorkspaceId, d: Dream) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&d.id))
            .ok_or(StoreError::NotFound)?;
        e.0 = d;
        Ok(())
    }

    async fn list(&self, ws: &WorkspaceId) -> Result<Vec<Dream>, StoreError> {
        Ok(self
            .inner
            .read()
            .get(ws)
            .map(|m| m.values().map(|(d, _)| d.clone()).collect())
            .unwrap_or_default())
    }

    async fn push_run(&self, ws: &WorkspaceId, run: DreamRun) -> Result<(), StoreError> {
        let mut g = self.inner.write();
        let e = g
            .get_mut(ws)
            .and_then(|m| m.get_mut(&run.dream_id))
            .ok_or(StoreError::NotFound)?;
        e.1.push(run);
        Ok(())
    }

    async fn runs(&self, ws: &WorkspaceId, id: &DreamId) -> Result<Vec<DreamRun>, StoreError> {
        let g = self.inner.read();
        let e = g
            .get(ws)
            .and_then(|m| m.get(id))
            .ok_or(StoreError::NotFound)?;
        let mut out = e.1.clone();
        out.reverse();
        Ok(out)
    }
}
