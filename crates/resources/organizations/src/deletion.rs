//! `agentd organization delete`: the store removes the rows in one transaction; then the secrets,
//! blobs and dream schedules outside the database go, and anything that could not is named.

use std::sync::Arc;

use shared::OrgId;
use shared::ports::blobs::{BlobError, BlobStore};
use shared::ports::secrets::SecretStore;
use shared::ports::{SchedulerError, SchedulerPort};

use crate::registry::{OrganizationsStore, Removed, StoreError};

/// What a delete removed, and what outside the database it left.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Deleted {
    /// The rows and keys the transaction removed.
    pub removed: Removed,
    /// Each secret, blob or schedule removed after the commit, as `<kind> <key>`.
    pub gone: Vec<String>,
    /// Each secret, blob or schedule left, as `<kind> <key>: <error>`.
    pub left: Vec<String>,
}

/// Deletes organizations with everything they own.
pub struct OrganizationDeletion {
    store: Arc<dyn OrganizationsStore>,
    secrets: Arc<dyn SecretStore>,
    blobs: Arc<dyn BlobStore>,
    scheduler: Arc<dyn SchedulerPort>,
}

impl std::fmt::Debug for OrganizationDeletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrganizationDeletion").finish_non_exhaustive()
    }
}

impl OrganizationDeletion {
    /// Over the rows in `store` and the stores outside it.
    pub fn new(
        store: Arc<dyn OrganizationsStore>,
        secrets: Arc<dyn SecretStore>,
        blobs: Arc<dyn BlobStore>,
        scheduler: Arc<dyn SchedulerPort>,
    ) -> Self {
        Self { store, secrets, blobs, scheduler }
    }

    /// Delete `org`; once its rows are gone, remove what they named outside the database.
    pub async fn delete(&self, org: &OrgId) -> Result<Deleted, StoreError> {
        let removed = self.store.delete(org).await?;
        let mut outcomes = vec![];
        for key in &removed.secrets {
            outcomes
                .push((format!("secret {key}"), self.secrets.delete(key).await.map_err(|e| e.to_string())));
        }
        for key in &removed.blobs {
            let deleted = match self.blobs.delete(key).await {
                Ok(()) | Err(BlobError::NotFound(_)) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
            outcomes.push((format!("blob {key}"), deleted));
        }
        for target in &removed.dreams {
            let deleted = match self.scheduler.delete(target).await {
                Ok(()) | Err(SchedulerError::Unknown(_)) => Ok(()),
                Err(e) => Err(e.to_string()),
            };
            outcomes.push((format!("schedule {}", target.key()), deleted));
        }
        let (mut gone, mut left) = (vec![], vec![]);
        for (what, deleted) in outcomes {
            match deleted {
                Ok(()) => gone.push(what),
                Err(e) => left.push(format!("{what}: {e}")),
            }
        }
        Ok(Deleted { removed, gone, left })
    }
}

#[cfg(test)]
mod tests;
