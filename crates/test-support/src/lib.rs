//! test-support: in-memory fakes of agentd's security ports, for tests only.
//! This crate is a dev-dependency of the crates that test against it and is
//! never a dependency of the binary, so no configuration can select a fake.

use std::collections::{BTreeMap, HashMap};

use async_trait::async_trait;
use parking_lot::Mutex;
use shared::ports::keys::{AdminKeySource, EnvironmentGrant, EnvironmentKeySource, KeyError, KeySource};
use shared::ports::secrets::{ExposeSecret, SecretError, SecretKey, SecretStore, SecretString};
use shared::{AdminContext, AuthContext, OrgId, WorkspaceId};

/// Secrets held in memory, by key, with the keys put as `environment_variable` credentials'.
#[derive(Debug, Default)]
pub struct MemorySecretStore(
    Mutex<BTreeMap<String, SecretString>>,
    Mutex<std::collections::BTreeSet<String>>,
    std::sync::atomic::AtomicUsize,
);

impl MemorySecretStore {
    /// How many secrets have been read.
    pub fn reads(&self) -> usize {
        self.2.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether `key`'s secret was put as an `environment_variable` credential's, which AWS tags.
    pub fn tagged(&self, key: &SecretKey) -> bool {
        self.1.lock().contains(&key.to_string())
    }

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
        if key.is_environment_variable() {
            self.1.lock().insert(key.to_string());
        }
        self.0.lock().insert(key.to_string(), secret.clone());
        Ok(())
    }

    async fn get(&self, key: &SecretKey) -> Result<SecretString, SecretError> {
        self.2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

/// Admin keys registered in memory.
#[derive(Debug, Default)]
pub struct StaticAdminKeys(HashMap<String, AdminContext>);

impl StaticAdminKeys {
    /// Accept `key` for `org`, as `admin_key_id`.
    pub fn register(&mut self, key: &str, org: &str, admin_key_id: &str) {
        let ctx = AdminContext { org: OrgId::new(org), admin_key_id: admin_key_id.to_owned() };
        self.0.insert(key.to_owned(), ctx);
    }
}

#[async_trait]
impl AdminKeySource for StaticAdminKeys {
    async fn resolve(&self, key: &str) -> Result<Option<AdminContext>, KeyError> {
        Ok(self.0.get(key).cloned())
    }
}

/// No environment keys: every one is unknown.
#[derive(Debug, Default)]
pub struct NoEnvironmentKeys;

#[async_trait]
impl EnvironmentKeySource for NoEnvironmentKeys {
    async fn resolve(&self, _: &str) -> Result<Option<EnvironmentGrant>, KeyError> {
        Ok(None)
    }
}
