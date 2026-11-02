//! The environment-variable credentials of a session's vaults, with their
//! values, for the egress proxy only: nothing here is ever serialized.

use std::sync::Arc;

use shared::{ApiError, WorkspaceId};

use crate::store::{StoreError, VaultsStore};
use crate::types::{CredentialAuthInput, CredentialAuthWire, CredentialNetworking, SecretString, VaultId};

/// One environment-variable credential a session may use.
#[derive(Debug, Clone)]
pub struct EnvSecret {
    /// The credential's id, for audit.
    pub credential_id: String,
    /// The variable name the sandbox sees.
    pub name: String,
    /// What it is for, from its display name.
    pub description: String,
    /// Hosts it may be sent to; `None` when unrestricted.
    pub hosts: Option<Vec<String>>,
    /// Whether it is put into request headers.
    pub header: bool,
    /// The value.
    pub value: SecretString,
}

/// Reads credentials with their values, for egress.
#[derive(Clone)]
pub struct VaultSecrets {
    store: Arc<dyn VaultsStore>,
}

impl std::fmt::Debug for VaultSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultSecrets")
    }
}

fn backend(e: StoreError) -> ApiError {
    ApiError::internal(e.to_string())
}

impl VaultSecrets {
    /// Read from `store`.
    pub fn new(store: Arc<dyn VaultsStore>) -> Self {
        Self { store }
    }

    /// The live environment-variable credentials of `vault_ids`, in order; a
    /// name in an earlier vault wins, as for every vault credential.
    pub async fn environment_secrets(
        &self,
        ws: &WorkspaceId,
        vault_ids: &[String],
    ) -> Result<Vec<EnvSecret>, ApiError> {
        let mut out: Vec<EnvSecret> = Vec::new();
        for raw in vault_ids {
            let Ok(vault) = VaultId::parse(raw) else { continue };
            let listed = match self.store.list_credentials(ws, &vault).await {
                Ok(l) => l,
                Err(StoreError::VaultNotFound) => continue,
                Err(e) => return Err(backend(e)),
            };
            for stored in listed {
                let c = stored.wire;
                let CredentialAuthWire::EnvironmentVariable { secret_name, networking, injection_location } =
                    &c.auth
                else {
                    continue;
                };
                if c.archived_at.is_some() || out.iter().any(|s| &s.name == secret_name) {
                    continue;
                }
                let full = self.store.get_credential(ws, &vault, &c.id).await.map_err(backend)?;
                let Some(CredentialAuthInput::EnvironmentVariable { secret_value, .. }) = full.auth else {
                    continue;
                };
                out.push(EnvSecret {
                    credential_id: c.id.as_str().to_owned(),
                    name: secret_name.clone(),
                    description: c.display_name.clone(),
                    hosts: match networking {
                        CredentialNetworking::Unrestricted => None,
                        CredentialNetworking::Limited { allowed_hosts } => Some(allowed_hosts.clone()),
                    },
                    header: injection_location.header,
                    value: secret_value,
                });
            }
        }
        Ok(out)
    }
}
