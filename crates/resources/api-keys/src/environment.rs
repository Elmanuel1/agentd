//! Issue, list, revoke and resolve environment keys. A self-hosted
//! environment's key is digested with the same peppers as workspace keys and
//! shown once. agentd's own runtime keys come from its configuration
//! (`AGENTD_RUNTIME_KEYS`), and only their digests are held here.

use std::sync::Arc;

use async_trait::async_trait;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::{EnvironmentGrant, EnvironmentKeySource, KeyError, PepperRing};
use shared::ports::secrets::{ExposeSecret, SecretString};
use shared::tenancy::RUNTIME_QUEUE;
use shared::{AuthContext, EnvironmentCaller, Id, OrgId, Timestamp, WorkspaceId};

use crate::service::{find, key_error, mint};
use crate::store::EnvironmentKeysStore;
use crate::types::{EnvironmentKey, EnvironmentKeyResource};

/// Environment keys over a store, digested with a pepper ring.
#[derive(Clone)]
pub struct EnvironmentKeysService {
    store: Arc<dyn EnvironmentKeysStore>,
    peppers: Arc<PepperRing>,
    runtime: Vec<Vec<u8>>,
}

impl std::fmt::Debug for EnvironmentKeysService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentKeysService").finish_non_exhaustive()
    }
}

impl EnvironmentKeysService {
    /// Keys in `store`, digested with `peppers`.
    pub fn new(store: Arc<dyn EnvironmentKeysStore>, peppers: Arc<PepperRing>) -> Self {
        Self { store, peppers, runtime: Vec::new() }
    }

    /// Also accept agentd's runtime keys, `runtime` (the current one, then
    /// those still in use), for the runtime queue.
    #[must_use]
    pub fn with_runtime_keys(self, runtime: &[SecretString]) -> Self {
        let current = self.peppers.current();
        let runtime = runtime.iter().filter_map(|k| material::digest(current, k.expose_secret())).collect();
        Self { runtime, ..self }
    }

    /// A new key for `environment` in `org` and `workspace`, all in the id
    /// charset. The key is in the return value only: it is not stored and
    /// cannot be read again.
    pub async fn create(
        &self,
        org: &str,
        workspace: &str,
        environment: &str,
        name: &str,
    ) -> Result<(EnvironmentKey, SecretString), KeyError> {
        // A key bound to one of agentd's own queues would reach other workspaces' work.
        if environment == RUNTIME_QUEUE {
            return Err(KeyError(format!(
                "`{environment}` names one of agentd's own queues, not an environment"
            )));
        }
        let (key, digest, hint) =
            mint(&self.peppers, KeyKind::Environment, &[org, workspace, environment], name)?;
        let record = EnvironmentKey {
            id: Id::<EnvironmentKeyResource>::generate().to_string(),
            org: OrgId::new(org),
            workspace: WorkspaceId::new(workspace),
            environment_id: environment.into(),
            name: name.into(),
            partial_key_hint: hint,
            created_at: Timestamp::now(),
            revoked_at: None,
        };
        self.store.insert(&record, &digest).await.map_err(key_error)?;
        Ok((record, key))
    }

    /// Every key, revoked ones included.
    pub async fn list(&self) -> Result<Vec<EnvironmentKey>, KeyError> {
        self.store.list().await.map_err(key_error)
    }

    /// Revoke `id`; `false` when no active key has it.
    pub async fn revoke(&self, id: &str) -> Result<bool, KeyError> {
        self.store.revoke(id, Timestamp::now()).await.map_err(key_error)
    }
}

#[async_trait]
impl EnvironmentKeySource for EnvironmentKeysService {
    async fn resolve(&self, key: &str) -> Result<Option<EnvironmentGrant>, KeyError> {
        if !material::well_formed(KeyKind::Environment, key) {
            return Ok(None);
        }
        if self.runtime.iter().any(|d| material::matches(self.peppers.current(), key, d)) {
            return Ok(Some(EnvironmentGrant::Runtime));
        }
        let found = find(&*self.store, &self.peppers, key).await?;
        Ok(found.map(|k| {
            let ctx = AuthContext { org: k.org, workspace: k.workspace, api_key_id: k.id };
            EnvironmentGrant::Environment(ctx, EnvironmentCaller { queue: k.environment_id })
        }))
    }
}

#[cfg(test)]
mod tests;
