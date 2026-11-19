//! A deployment write that never overwrites a newer one (DS-14): every
//! read-change-write of a deployment goes through [`DeploymentsService::rewrite`].

use shared::ports::ScheduleChange;
use shared::{ApiError, AuthContext, Timestamp};

use super::{DeploymentsService, map_store};
use crate::store::StoreError;
use crate::types::{Deployment, DeploymentId};

/// Reads of one deployment before a change gives up on it changing under it.
const ATTEMPTS: usize = 5;

impl DeploymentsService {
    /// Read deployment `id`, apply `change` and write the result, with the
    /// schedule change `change` returns, only while the deployment is as it
    /// was read; a deployment changed since is read again and changed again.
    /// A change that leaves the deployment as it was writes nothing.
    pub(super) async fn rewrite(
        &self,
        ctx: &AuthContext,
        id: &DeploymentId,
        mut change: impl FnMut(&mut Deployment) -> Result<Option<ScheduleChange>, ApiError>,
    ) -> Result<Deployment, ApiError> {
        for _ in 0..ATTEMPTS {
            let read = self.store.get(&ctx.workspace, id).await.map_err(map_store)?;
            let mut d = read.clone();
            let schedule = change(&mut d)?;
            if schedule.is_none() && d == read {
                return Ok(d);
            }
            d.updated_at = Timestamp::now();
            match self.store.replace(&ctx.workspace, d.clone(), read.updated_at, schedule.as_ref()).await {
                Err(StoreError::Stale) => continue,
                written => return written.map(|()| d).map_err(map_store),
            }
        }
        Err(map_store(StoreError::Stale))
    }
}

#[cfg(test)]
mod tests;
