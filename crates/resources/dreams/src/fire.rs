//! Dreams' scheduled work: each tick fires the dream through
//! [`DreamsService::fire`], under the schedule's tenant.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use shared::ports::fire::{FireError, FireHandler};
use shared::ports::FireTarget;
use shared::AuthContext;

use crate::service::DreamsService;
use crate::types::DreamId;

/// The [`FireTarget::kind`] of dreams.
pub const KIND: &str = "dream";

/// The schedule target of dream `id`.
pub fn target(id: &DreamId) -> FireTarget {
    FireTarget::new(KIND, id.as_str())
}

/// Fires scheduled dreams.
pub struct DreamFire {
    service: Arc<DreamsService>,
}

impl std::fmt::Debug for DreamFire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DreamFire").finish_non_exhaustive()
    }
}

impl DreamFire {
    /// Fire through `service`.
    pub fn new(service: Arc<DreamsService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl FireHandler for DreamFire {
    fn kind(&self) -> &'static str {
        KIND
    }

    async fn fire(
        &self,
        tenant: &AuthContext,
        id: &str,
        _scheduled_at: DateTime<Utc>,
    ) -> Result<(), FireError> {
        self.service.fire(tenant, id, false).await.map(|_| ()).map_err(|e| FireError::Failed(e.to_string()))
    }
}
