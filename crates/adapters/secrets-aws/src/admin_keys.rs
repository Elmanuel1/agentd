//! Admin keys in Secrets Manager: the [`AdminKeySource`] every
//! `/v1/organizations` request resolves through, and the create, list and
//! revoke `agentd admin-keys` runs.
//!
//! Each key is one secret, `<prefix>/system/admin_keys/<digest>`, where the
//! digest is the key's HMAC-SHA256 under the pepper ([`material`]), hex
//! encoded. Its value holds the digest, the `adminkey_…` id and the org;
//! never the key, which is returned once by `AwsAdminKeys::create`.
//! Its tags carry what a listing shows. It is encrypted under the same
//! customer-managed KMS key as every credential. A revoke deletes the secret,
//! and nothing is cached, so a revoke holds from the next request.

mod manage;

use std::sync::Arc;

use shared::ports::keys::material::{self, KeyKind};
use shared::ports::keys::{AdminKeySource, KeyError, KeyPepper};
use shared::ports::secrets::{ExposeSecret, SecretError};
use shared::{AdminContext, OrgId, Resource};

use crate::AwsSecretStore;

const DIR: &str = "system/admin_keys";
/// What errors call a key's secret, whose name would identify the key.
const SHOWN: &str = "an admin key secret";

/// Marker for `adminkey_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct AdminKeyResource;
impl Resource for AdminKeyResource {
    const PREFIX: &'static str = "adminkey";
    const LABEL: &'static str = "admin key";
}

/// One admin key as a listing shows it, without its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminKeyRecord {
    /// `adminkey_…`.
    pub id: String,
    /// The organization it administers.
    pub org: String,
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
    admin_key_id: String,
    org: String,
}

/// Admin keys in Secrets Manager, digested with one pepper.
#[derive(Clone)]
pub struct AwsAdminKeys {
    store: AwsSecretStore,
    pepper: Arc<KeyPepper>,
}

impl std::fmt::Debug for AwsAdminKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsAdminKeys").field("prefix", &self.store.prefix).finish_non_exhaustive()
    }
}

fn key_error(e: SecretError) -> KeyError {
    KeyError(e.to_string())
}

impl AwsAdminKeys {
    /// Keys under `store`'s prefix and KMS key, digested with `pepper`.
    pub fn new(store: AwsSecretStore, pepper: Arc<KeyPepper>) -> Self {
        Self { store, pepper }
    }

    fn dir(&self) -> String {
        format!("{}/{DIR}/", self.store.prefix)
    }
}

#[async_trait::async_trait]
impl AdminKeySource for AwsAdminKeys {
    async fn resolve(&self, key: &str) -> Result<Option<AdminContext>, KeyError> {
        if !material::well_formed(KeyKind::Admin, key) {
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
        Ok(Some(AdminContext { org: OrgId::new(entry.org), admin_key_id: entry.admin_key_id }))
    }
}
