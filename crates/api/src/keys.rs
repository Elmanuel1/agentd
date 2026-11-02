//! The `env` [`KeySource`]: the API keys listed in configuration, held in
//! memory and compared in plain text. For development and tests.

use std::collections::HashMap;

use async_trait::async_trait;
use shared::ports::keys::KeySource;
use shared::{AuthContext, OrgId, WorkspaceId};

use crate::config::ApiKey;

/// API keys from configuration.
#[derive(Debug, Clone, Default)]
pub struct EnvKeySource {
    keys: HashMap<String, AuthContext>,
}

impl EnvKeySource {
    /// The configured `keys`.
    pub fn new(keys: &[ApiKey]) -> Self {
        let mut source = Self::default();
        for k in keys {
            source.register(&k.key, &k.org, &k.workspace, &k.api_key_id);
        }
        source
    }

    /// Add `key`, bound to `org` and `workspace`.
    pub fn register(&mut self, key: &str, org: &str, workspace: &str, api_key_id: &str) {
        self.keys.insert(
            key.to_owned(),
            AuthContext {
                org: OrgId::new(org),
                workspace: WorkspaceId::new(workspace),
                api_key_id: api_key_id.to_owned(),
            },
        );
    }
}

#[async_trait]
impl KeySource for EnvKeySource {
    async fn resolve(&self, key: &str) -> Option<AuthContext> {
        self.keys.get(key).cloned()
    }
}
