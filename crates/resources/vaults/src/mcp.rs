//! The loop's reads of a session's MCP credentials: which credential each
//! server connects with, and its token, read once in each step that uses it
//! so a rotated, archived or deleted credential applies from the next step.

use std::collections::HashMap;

use shared::{BackendError, Timestamp, WorkspaceId};

use crate::service::VaultsService;
use crate::store::StoreError;
use crate::types::{Credential, CredentialAuthInput, CredentialAuthWire, SecretString, VaultId};

/// What a server is called with.
#[derive(Debug, Clone)]
pub struct McpToken {
    /// The bearer token.
    pub bearer: SecretString,
    /// An `mcp_oauth` token past its `expires_at`.
    pub expired: bool,
}

impl VaultsService {
    /// The credential each of `urls` connects with: the first of `vault_ids`
    /// that holds an unarchived one keyed by that URL (B10). A URL with none
    /// connects unauthenticated (MC-3).
    pub async fn mcp_credentials(
        &self,
        ws: &WorkspaceId,
        vault_ids: &[String],
        urls: &[String],
    ) -> Result<HashMap<String, Credential>, BackendError> {
        if vault_ids.is_empty() || urls.is_empty() {
            return Ok(HashMap::new());
        }
        let found = self.credentials.mcp_credentials(ws, vault_ids, urls).await.map_err(BackendError::new)?;
        let mut chosen: HashMap<String, Credential> = HashMap::new();
        for cred in found {
            let Some(url) = server_url(&cred) else { continue };
            let rank = |c: &Credential| vault_ids.iter().position(|v| *v == c.vault_id);
            if chosen.get(&url).is_none_or(|held| rank(&cred) < rank(held)) {
                chosen.insert(url, cred);
            }
        }
        Ok(chosen)
    }

    /// `cred`'s token; `None` when its secret is gone (archived or deleted
    /// since the credential was read).
    pub async fn mcp_token(
        &self,
        ws: &WorkspaceId,
        cred: &Credential,
    ) -> Result<Option<McpToken>, BackendError> {
        let vault = VaultId::parse(&cred.vault_id).map_err(BackendError::new)?;
        match self.credentials.credential_secret(ws, &vault, &cred.id).await {
            Ok(CredentialAuthInput::McpOauth { access_token, expires_at, .. }) => Ok(Some(McpToken {
                bearer: access_token,
                expired: expires_at.is_some_and(|at| at <= Timestamp::now()),
            })),
            Ok(CredentialAuthInput::StaticBearer { token, .. }) => {
                Ok(Some(McpToken { bearer: token, expired: false }))
            }
            Ok(CredentialAuthInput::EnvironmentVariable { .. }) => Ok(None),
            Err(StoreError::CredentialNotFound) => Ok(None),
            Err(e) => Err(BackendError::new(e)),
        }
    }
}

fn server_url(cred: &Credential) -> Option<String> {
    match &cred.auth {
        CredentialAuthWire::McpOauth { mcp_server_url, .. }
        | CredentialAuthWire::StaticBearer { mcp_server_url } => Some(mcp_server_url.clone()),
        CredentialAuthWire::EnvironmentVariable { .. } => None,
    }
}
