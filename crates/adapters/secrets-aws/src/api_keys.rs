//! API keys in Secrets Manager: the [`KeySource`] every request resolves
//! through, and the create, list and revoke `agentd keys` runs.
//!
//! Each key is one secret, `<prefix>/system/api_keys/<digest>`, where the
//! digest is the key's HMAC-SHA256 under the pepper ([`material`]), hex
//! encoded. Its value holds the digest, the `apikey_…` id, the org and the
//! workspace; never the key, which is returned once by `AwsApiKeys::create`.
//! Its tags carry what a listing shows. It is encrypted under the same
//! customer-managed KMS key as every credential. A revoke deletes the secret,
//! and nothing is cached, so a revoke holds from the next request.

mod admin;
mod material;

use std::sync::Arc;

use shared::ports::keys::{KeyError, KeyPepper, KeySource};
use shared::ports::secrets::{ExposeSecret, SecretError};
use shared::{AuthContext, OrgId, Resource, WorkspaceId};

use crate::AwsSecretStore;

const DIR: &str = "system/api_keys";
/// What errors call a key's secret, whose name would identify the key.
const SHOWN: &str = "an API key secret";

/// Marker for `apikey_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct ApiKeyResource;
impl Resource for ApiKeyResource {
    const PREFIX: &'static str = "apikey";
    const LABEL: &'static str = "API key";
}

/// One API key as a listing shows it, without its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyRecord {
    /// `apikey_…`.
    pub id: String,
    /// The organization it acts for.
    pub org: String,
    /// The workspace it acts in.
    pub workspace: String,
    /// A label for people.
    pub name: String,
    /// Its first 16 and last 4 characters.
    pub partial_key_hint: String,
    /// When it was created, RFC 3339.
    pub created_at: String,
    /// Whether it was revoked; it resolves to no one from then on.
    pub revoked: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    digest: String,
    api_key_id: String,
    org: String,
    workspace: String,
}

/// API keys in Secrets Manager, digested with one pepper.
#[derive(Clone)]
pub struct AwsApiKeys {
    store: AwsSecretStore,
    pepper: Arc<KeyPepper>,
}

impl std::fmt::Debug for AwsApiKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsApiKeys").field("prefix", &self.store.prefix).finish_non_exhaustive()
    }
}

fn key_error(e: SecretError) -> KeyError {
    KeyError(e.to_string())
}

impl AwsApiKeys {
    /// Keys under `store`'s prefix and KMS key, digested with `pepper`.
    pub fn new(store: AwsSecretStore, pepper: Arc<KeyPepper>) -> Self {
        Self { store, pepper }
    }

    fn dir(&self) -> String {
        format!("{}/{DIR}/", self.store.prefix)
    }
}

#[async_trait::async_trait]
impl KeySource for AwsApiKeys {
    async fn resolve(&self, key: &str) -> Result<Option<AuthContext>, KeyError> {
        if !material::well_formed(key) {
            return Ok(None);
        }
        let Some(digest) = material::digest(&self.pepper, key) else {
            return Ok(None);
        };
        let value = match self.store.read(&format!("{}{}", self.dir(), hex::encode(digest)), SHOWN).await {
            Ok(value) => value,
            Err(SecretError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(key_error(e)),
        };
        let entry: Entry = serde_json::from_str(value.expose_secret())
            .map_err(|_| KeyError(format!("{SHOWN} is unreadable")))?;
        // The name found the secret; the constant-time check decides it.
        let stored = hex::decode(&entry.digest).unwrap_or_default();
        if !material::matches(&self.pepper, key, &stored) {
            return Ok(None);
        }
        Ok(Some(AuthContext {
            org: OrgId::new(entry.org),
            workspace: WorkspaceId::new(entry.workspace),
            api_key_id: entry.api_key_id,
        }))
    }
}
