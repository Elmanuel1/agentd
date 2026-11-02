//! Deployments' scheduled work: each tick fires the deployment through
//! [`DeploymentsService::fire_scheduled`], under the schedule's tenant.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shared::ports::fire::{FireError, FireHandler};
use shared::ports::FireTarget;
use shared::{AuthContext, Timestamp};

use crate::service::DeploymentsService;
use crate::types::DeploymentId;

/// The [`FireTarget::kind`] of deployments.
pub const KIND: &str = "deployment";

/// The schedule target of deployment `id`.
pub fn target(id: &DeploymentId) -> FireTarget {
    FireTarget::new(KIND, id.as_str())
}

/// Fires scheduled deployments.
pub struct DeploymentFire {
    service: Arc<DeploymentsService>,
}

impl std::fmt::Debug for DeploymentFire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentFire").finish_non_exhaustive()
    }
}

impl DeploymentFire {
    /// Fire through `service`.
    pub fn new(service: Arc<DeploymentsService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl FireHandler for DeploymentFire {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn fire(
        &self,
        tenant: &AuthContext,
        id: &str,
        scheduled_at: DateTime<Utc>,
    ) -> Result<(), FireError> {
        self.service
            .fire_scheduled(tenant, id, Timestamp::from_datetime(scheduled_at))
            .await
            .map(|_| ())
            .map_err(|e| FireError::Failed(e.to_string()))
    }
}
