//! test-support: in-memory fakes of agentd's security ports, for tests only.
//! This crate is a dev-dependency of the crates that test against it and is
//! never a dependency of the binary, so no configuration can select a fake.

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use parking_lot::Mutex;
use shared::ports::keys::{KeyError, KeySource};
use shared::ports::secrets::{ExposeSecret, SecretError, SecretKey, SecretStore, SecretString};
use shared::{AuthContext, OrgId, WorkspaceId};

/// Secrets held in memory, by key.
#[derive(Debug, Default)]
pub struct MemorySecretStore(Mutex<BTreeMap<String, SecretString>>);

impl MemorySecretStore {
    /// The value at `key`, as the store holds it.
    pub fn exposed(&self, key: &SecretKey) -> Option<String> {
        self.0.lock().get(&key.to_string()).map(|s| s.expose_secret().to_owned())
    }

    /// How many secrets the store holds.
    pub fn len(&self) -> usize {
        self.0.lock().len()
    }

    /// Whether the store holds none.
    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }
}

#[async_trait]
impl SecretStore for MemorySecretStore {
    async fn put(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretError> {
        self.0.lock().insert(key.to_string(), secret.clone());
        Ok(())
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        self.0.lock().get(&key.to_string()).cloned().ok_or_else(|| SecretError::NotFound(key.to_string()))
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        self.0.lock().remove(&key.to_string());
        Ok(())
    }
}

/// API keys registered in memory.
#[derive(Debug, Default)]
pub struct StaticKeySource(HashMap<String, AuthContext>);

impl StaticKeySource {
    /// Accept `key` for `org` and `workspace`, as `api_key_id`.
    pub fn register(&mut self, key: &str, org: &str, workspace: &str, api_key_id: &str) {
        let ctx = AuthContext {
            org: OrgId::new(org),
            workspace: WorkspaceId::new(workspace),
            api_key_id: api_key_id.to_owned(),
        };
        self.0.insert(key.to_owned(), ctx);
    }
}

#[async_trait]
impl KeySource for StaticKeySource {
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError> {
        Ok(self.0.get(key).cloned())
    }
}
