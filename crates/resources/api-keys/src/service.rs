//! Issue, list, revoke and resolve workspace API keys.

use std::sync::Arc;

use async_trait::async_trait;
use shared::charset::is_id_charset;
use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::{KeyError, KeyPepper, KeySource};
use shared::ports::secrets::{ExposeSecret, SecretString};
use shared::{AuthContext, Id, OrgId, Timestamp, WorkspaceId};

use crate::store::{ApiKeysStore, StoreError};
use crate::types::{ApiKey, ApiKeyResource};

/// Workspace API keys over a store, digested with one pepper.
#[derive(Clone)]
pub struct ApiKeysService {
    store: Arc<dyn ApiKeysStore>,
    pepper: Arc<KeyPepper>,
}

impl std::fmt::Debug for ApiKeysService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKeysService").finish_non_exhaustive()
    }
}

pub(crate) fn key_error(e: StoreError) -> KeyError {
    KeyError(e.to_string())
}

/// A name for people: 1 to 128 characters, none of them a control character.
fn name_ok(name: &str) -> bool {
    (1..=128).contains(&name.chars().count()) && !name.chars().any(char::is_control)
}

/// A new key of `kind`, its stored digest and its hint, once `ids` are in the
/// id charset and `name` is a name for people.
pub(crate) fn mint(
    pepper: &KeyPepper,
    kind: KeyKind,
    ids: &[&str],
    name: &str,
) -> Result<(SecretString, Vec<u8>, String), KeyError> {
    if !ids.iter().all(|id| is_id_charset(id)) {
        return Err(KeyError("every id must be in the id charset".into()));
    }
    if !name_ok(name) {
        return Err(KeyError("the name must be 1 to 128 characters, with no control characters".into()));
    }
    let key = material::generate(kind);
    let digest = material::digest(pepper, key.expose_secret())
        .ok_or_else(|| KeyError("the pepper cannot key a digest".into()))?;
    let hint = material::hint(key.expose_secret());
    Ok((key, digest, hint))
}

impl ApiKeysService {
    /// Keys in `store`, digested with `pepper`.
    pub fn new(store: Arc<dyn ApiKeysStore>, pepper: Arc<KeyPepper>) -> Self {
        Self { store, pepper }
    }

    /// A new key for `org` and `workspace`, both in the id charset. The key is
    /// in the return value only: it is not stored and cannot be read again.
    pub async fn create(
        &self,
        org: &str,
        workspace: &str,
        name: &str,
    ) -> Result<(ApiKey, SecretString), KeyError> {
        let (key, digest, hint) = mint(&self.pepper, KeyKind::Workspace, &[org, workspace], name)?;
        let record = ApiKey {
            id: Id::<ApiKeyResource>::generate().to_string(),
            org: OrgId::new(org),
            workspace: WorkspaceId::new(workspace),
            name: name.into(),
            partial_key_hint: hint,
            created_at: Timestamp::now(),
            revoked_at: None,
        };
        self.store.insert(&record, &digest).await.map_err(key_error)?;
        Ok((record, key))
    }

    /// Every key, revoked ones included.
    pub async fn list(&self) -> Result<Vec<ApiKey>, KeyError> {
        self.store.list().await.map_err(key_error)
    }

    /// Revoke `id`; `false` when no active key has it.
    pub async fn revoke(&self, id: &str) -> Result<bool, KeyError> {
        self.store.revoke(id, Timestamp::now()).await.map_err(key_error)
    }
}

#[async_trait]
impl KeySource for ApiKeysService {
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError> {
        if !material::well_formed(KeyKind::Workspace, key) {
            return Ok(None);
        }
        let Some(digest) = material::digest(&self.pepper, key) else {
            return Ok(None);
        };
        let found = self.store.find_active(&digest).await.map_err(key_error)?;
        Ok(found.map(|k| AuthContext { org: k.org, workspace: k.workspace, api_key_id: k.id }))
    }
}

#[cfg(test)]
mod tests;
