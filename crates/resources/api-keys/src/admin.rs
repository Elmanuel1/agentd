//! Issue, list, revoke and resolve organization admin keys. An admin key is
//! digested with the same peppers as workspace keys, shown once, and reaches
//! only the `/v1/organizations` routes of its organization.

use std::sync::Arc;

use async_trait::async_trait;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::{AdminKeySource, KeyError, PepperRing};
use shared::ports::secrets::SecretString;
use shared::{AdminContext, Id, OrgId, Timestamp};

use crate::service::{find, key_error, mint};
use crate::store::AdminKeysStore;
use crate::types::{AdminKey, AdminKeyResource};

/// Admin keys over a store, digested with a pepper ring.
#[derive(Clone)]
pub struct AdminKeysService {
    store: Arc<dyn AdminKeysStore>,
    peppers: Arc<PepperRing>,
}

impl std::fmt::Debug for AdminKeysService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminKeysService").finish_non_exhaustive()
    }
}

impl AdminKeysService {
    /// Keys in `store`, digested with `peppers`.
    pub fn new(store: Arc<dyn AdminKeysStore>, peppers: Arc<PepperRing>) -> Self {
        Self { store, peppers }
    }

    /// A new key for `org`, in the id charset. The key is in the return value
    /// only: it is not stored and cannot be read again.
    pub async fn create(&self, org: &str, name: &str) -> Result<(AdminKey, SecretString), KeyError> {
        let (key, digest, hint) = mint(&self.peppers, KeyKind::Admin, &[org], name)?;
        let record = AdminKey {
            id: Id::<AdminKeyResource>::generate().to_string(),
            org: OrgId::new(org),
            name: name.into(),
            partial_key_hint: hint,
            created_at: Timestamp::now(),
            revoked_at: None,
        };
        self.store.insert(&record, &digest).await.map_err(key_error)?;
        Ok((record, key))
    }

    /// Every key, revoked ones included.
    pub async fn list(&self) -> Result<Vec<AdminKey>, KeyError> {
        self.store.list().await.map_err(key_error)
    }

    /// Revoke `id`; `false` when no active key has it.
    pub async fn revoke(&self, id: &str) -> Result<bool, KeyError> {
        self.store.revoke(id, Timestamp::now()).await.map_err(key_error)
    }
}

#[async_trait]
impl AdminKeySource for AdminKeysService {
    async fn resolve(&self, key: &str) -> Result<Option<AdminContext>, KeyError> {
        if !material::well_formed(KeyKind::Admin, key) {
            return Ok(None);
        }
        let found = find(&*self.store, &self.peppers, key).await?;
        Ok(found.map(|k| AdminContext { org: k.org, admin_key_id: k.id }))
    }
}
