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
        let mut left = vec![];
        for key in &removed.secrets {
            if let Err(e) = self.secrets.delete(key).await {
                left.push(format!("secret {key}: {e}"));
            }
        }
        for key in &removed.blobs {
            match self.blobs.delete(key).await {
                Ok(()) | Err(BlobError::NotFound(_)) => {}
                Err(e) => left.push(format!("blob {key}: {e}")),
            }
        }
        for target in &removed.dreams {
            match self.scheduler.delete(target).await {
                Ok(()) | Err(SchedulerError::Unknown(_)) => {}
                Err(e) => left.push(format!("schedule {}: {e}", target.key())),
            }
        }
        Ok(Deleted { removed, left })
    }
}

#[cfg(test)]
mod tests;
