//! The environment-variable credentials of a session's vaults, as the
//! sandbox and the egress proxy know them: names, hosts and where each
//! value is stored. No value is read here; only the egress service reads
//! one, when a request needs it.

use std::sync::Arc;

use shared::{ApiError, WorkspaceId};

use crate::store::{StoreError, VaultCredentialStore};
use crate::types::{CredentialAuthWire, CredentialNetworking, VaultId};

/// One environment-variable credential a session may use.
#[derive(Debug, Clone)]
pub struct EnvCredential {
    /// The credential's id, for audit.
    pub credential_id: String,
    /// Its vault's id.
    pub vault_id: String,
    /// The variable name the sandbox sees.
    pub name: String,
    /// What it is for, from its display name.
    pub description: String,
    /// Hosts it may be sent to; `None` when unrestricted.
    pub hosts: Option<Vec<String>>,
}

/// Lists the credentials a session's sandbox may use.
#[derive(Clone)]
pub struct VaultCredentials {
    store: Arc<dyn VaultCredentialStore>,
}

impl std::fmt::Debug for VaultCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultCredentials")
    }
}

fn backend(e: StoreError) -> ApiError {
    ApiError::internal(e.to_string())
}

impl VaultCredentials {
    /// Read from `store`.
    pub fn new(store: Arc<dyn VaultCredentialStore>) -> Self {
        Self { store }
    }

    /// The live environment-variable credentials of `vault_ids`, in order; a
    /// name in an earlier vault wins, as for every vault credential.
    pub async fn environment_credentials(
        &self,
        ws: &WorkspaceId,
        vault_ids: &[String],
    ) -> Result<Vec<EnvCredential>, ApiError> {
        let mut out: Vec<EnvCredential> = Vec::new();
        for raw in vault_ids {
            let Ok(vault) = VaultId::parse(raw) else { continue };
            let listed = match self.store.list_credentials(ws, &vault).await {
                Ok(l) => l,
                Err(StoreError::VaultNotFound) => continue,
                Err(e) => return Err(backend(e)),
            };
            for stored in listed {
                let c = stored.wire;
                let CredentialAuthWire::EnvironmentVariable { secret_name, networking, .. } = &c.auth else {
                    continue;
                };
                if c.archived_at.is_some() || out.iter().any(|s| &s.name == secret_name) {
                    continue;
                }
                out.push(EnvCredential {
                    credential_id: c.id.as_str().to_owned(),
                    vault_id: vault.as_str().to_owned(),
                    name: secret_name.clone(),
                    description: c.display_name.clone(),
                    hosts: match networking {
                        CredentialNetworking::Unrestricted => None,
                        CredentialNetworking::Limited { allowed_hosts } => Some(allowed_hosts.clone()),
                    },
                });
            }
        }
        Ok(out)
    }
}
