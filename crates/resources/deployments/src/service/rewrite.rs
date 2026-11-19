//! A deployment write that never overwrites a newer one (DS-14): every
//! read-change-write of a deployment goes through [`DeploymentsService::rewrite`].

use shared::ports::ScheduleChange;
use shared::{ApiError, AuthContext, Timestamp};

use super::{DeploymentsService, map_store};
use crate::store::StoreError;
use crate::types::Deployment;

/// Writes of one change before it gives up on a deployment that keeps changing.
const ATTEMPTS: usize = 5;

impl DeploymentsService {
    /// Apply `change` to `read`, the deployment as its caller read it, and
    /// write the result, with the schedule change `change` returns, only while
    /// the stored deployment is still `read`; a deployment changed since is
    /// read again and changed again. A change that leaves the deployment as it
    /// was writes nothing.
    pub(super) async fn rewrite(
        &self,
        ctx: &AuthContext,
        mut read: Deployment,
        mut change: impl FnMut(&mut Deployment) -> Result<Option<ScheduleChange>, ApiError>,
    ) -> Result<Deployment, ApiError> {
        for attempt in 1..=ATTEMPTS {
            let mut d = read.clone();
            let schedule = change(&mut d)?;
            if schedule.is_none() && d == read {
                return Ok(d);
            }
            d.updated_at = Timestamp::now();
            match self.store.replace(&ctx.workspace, d.clone(), read.updated_at, schedule.as_ref()).await {
                Err(StoreError::Stale) if attempt < ATTEMPTS => {
                    read = self.store.get(&ctx.workspace, &d.id).await.map_err(map_store)?;
                }
                written => return written.map(|()| d).map_err(map_store),
            }
        }
        Err(map_store(StoreError::Stale))
    }
}

#[cfg(test)]
mod tests;
