//! Issue, list, revoke and resolve environment keys. A self-hosted
//! environment's key is digested with the same pepper as workspace keys and
//! shown once; agentd's own runtime key lives in Secrets Manager, and only
//! its digest is held here.

use std::sync::Arc;

use async_trait::async_trait;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::{EnvironmentGrant, EnvironmentKeySource, KeyError, KeyPepper};
use shared::ports::secrets::{ExposeSecret, SecretString};
use shared::{AuthContext, EnvironmentCaller, Id, OrgId, Timestamp, WorkspaceId};

use crate::service::{key_error, mint};
use crate::store::EnvironmentKeysStore;
use crate::types::{EnvironmentKey, EnvironmentKeyResource};

/// The queues agentd keeps for itself (`work::RUNTIME`, `work::HOSTED`): a
/// key bound to one would reach other workspaces' work.
const RESERVED_QUEUES: [&str; 2] = ["runtime", "hosted"];

/// Environment keys over a store, digested with one pepper.
#[derive(Clone)]
pub struct EnvironmentKeysService {
    store: Arc<dyn EnvironmentKeysStore>,
    pepper: Arc<KeyPepper>,
    runtime: Option<Vec<u8>>,
}

impl std::fmt::Debug for EnvironmentKeysService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvironmentKeysService").finish_non_exhaustive()
    }
}

impl EnvironmentKeysService {
    /// Keys in `store`, digested with `pepper`.
    pub fn new(store: Arc<dyn EnvironmentKeysStore>, pepper: Arc<KeyPepper>) -> Self {
        Self { store, pepper, runtime: None }
    }

    /// Also accept agentd's runtime key, `runtime`, for the runtime queue.
    #[must_use]
    pub fn with_runtime_key(self, runtime: &SecretString) -> Self {
        let runtime = material::digest(&self.pepper, runtime.expose_secret());
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
        if RESERVED_QUEUES.contains(&environment) {
            return Err(KeyError(format!(
                "`{environment}` names one of agentd's own queues, not an environment"
            )));
        }
        let (key, digest, hint) =
            mint(&self.pepper, KeyKind::Environment, &[org, workspace, environment], name)?;
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
        if self.runtime.as_ref().is_some_and(|d| material::matches(&self.pepper, key, d)) {
            return Ok(Some(EnvironmentGrant::Runtime));
        }
        let Some(digest) = material::digest(&self.pepper, key) else {
            return Ok(None);
        };
        let found = self.store.find_active(&digest).await.map_err(key_error)?;
        Ok(found.map(|k| {
            let ctx = AuthContext { org: k.org, workspace: k.workspace, api_key_id: k.id };
            EnvironmentGrant::Environment(ctx, EnvironmentCaller { queue: k.environment_id })
        }))
    }
}

#[cfg(test)]
mod tests;
