//! The `env` [`KeySource`]: the API keys listed in `AGENTD_API_KEYS`, held in
//! memory and compared in plain text. For development and tests.

use std::collections::HashMap;

use async_trait::async_trait;
use shared::ports::keys::KeySource;
use shared::{AuthContext, OrgId, WorkspaceId};

use crate::config::{flag, ConfigError};

/// One API key and the tenant it is bound to.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey {
    /// The secret the caller sends.
    pub key: String,
    /// Its organization.
    pub org: String,
    /// Its workspace.
    pub workspace: String,
    /// Its `apikey_…` id, the audit actor of its writes.
    pub api_key_id: String,
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiKey")
            .field("org", &self.org)
            .field("workspace", &self.workspace)
            .field("api_key_id", &self.api_key_id)
            .finish_non_exhaustive()
    }
}

/// The key `AGENTD_DEV_KEY=1` registers.
pub const DEV_KEY: &str = "agentd-dev-key:org_dev:wrkspc_01dev";

/// `AGENTD_API_KEYS` is `key:org:workspace[,key:org:workspace…]`.
pub(crate) fn env_keys(get: &impl Fn(&str) -> Option<String>) -> Result<Vec<ApiKey>, ConfigError> {
    const VAR: &str = "AGENTD_API_KEYS";
    let mut triples: Vec<String> = get(VAR)
        .map(|v| v.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default();
    if flag(get, "AGENTD_DEV_KEY", false)? {
        triples.push(DEV_KEY.to_owned());
    }
    if triples.is_empty() {
        return Err(ConfigError {
            var: VAR,
            reason: "no API keys; set it, or AGENTD_DEV_KEY=1 in development".into(),
        });
    }
    triples
        .iter()
        .enumerate()
        .map(|(i, triple)| match triple.splitn(3, ':').collect::<Vec<_>>().as_slice() {
            [key, org, workspace] if !key.is_empty() && !org.is_empty() && !workspace.is_empty() => {
                Ok(ApiKey {
                    key: (*key).to_owned(),
                    org: (*org).to_owned(),
                    workspace: (*workspace).to_owned(),
                    api_key_id: format!("apikey_01local{i:010}"),
                })
            }
            _ => Err(ConfigError { var: VAR, reason: format!("entry {i} is not key:org:workspace") }),
        })
        .collect()
}

/// API keys from configuration.
#[derive(Clone, Default)]
pub struct EnvKeySource {
    keys: HashMap<String, AuthContext>,
}

impl std::fmt::Debug for EnvKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvKeySource").field("keys", &self.keys.len()).finish()
    }
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
