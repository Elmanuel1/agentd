//! The loop's reads of a session's MCP credentials: which credential each
//! server connects with, and its token, read once in each step that uses it
//! so a rotated, archived or deleted credential applies from the next step.

use std::collections::HashMap;

use shared::{BackendError, OrgId, Timestamp, WorkspaceId};

use crate::refresh::{request_token, Refreshed};
use crate::service::VaultsService;
use crate::store::{RefreshWrite, StoreError, StoredCredential};
use crate::types::{
    Credential, CredentialAuthInput, CredentialAuthWire, OauthRefreshInput, SecretString, VaultId,
};

/// How long a refresh claim holds: a row updated more recently than this
/// is another worker's refresh in flight (MC-20).
const CLAIM_HOLDS: i64 = 30;

/// What an expired token's refresh came to.
#[derive(Debug)]
pub enum Refresh {
    /// A token to call with: refreshed, or another worker's that is fresh,
    /// and the credential as it now reads.
    Token(McpToken, Box<Credential>),
    /// Another worker holds the refresh: the step runs again a second later.
    Busy,
    /// The refresh failed; the server is not called (MC-21, MC-27).
    Failed,
}

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

impl VaultsService {
    /// Refresh `cred`'s expired token, claimed by moving the row's
    /// `updated_at` from what the step read, so one worker refreshes (MC-7).
    pub async fn refresh_mcp_oauth(
        &self,
        ws: &WorkspaceId,
        org: &OrgId,
        cred: &Credential,
    ) -> Result<Refresh, BackendError> {
        let now = Timestamp::now();
        let held = now.as_datetime() - cred.updated_at.as_datetime() < chrono::Duration::seconds(CLAIM_HOLDS);
        if held {
            return Ok(Refresh::Busy);
        }
        let vault = VaultId::parse(&cred.vault_id).map_err(BackendError::new)?;
        let claimed = self
            .credentials
            .claim_refresh(ws, &vault, &cred.id, cred.updated_at, now)
            .await
            .map_err(BackendError::new)?;
        let stored = match self.credentials.get_credential(ws, &vault, &cred.id).await {
            Ok(stored) => stored,
            Err(StoreError::CredentialNotFound) => return Ok(Refresh::Failed),
            Err(e) => return Err(BackendError::new(e)),
        };
        let Some(CredentialAuthInput::McpOauth { mcp_server_url, expires_at, refresh, access_token }) =
            stored.auth
        else {
            return Ok(Refresh::Failed);
        };
        let fresh = expires_at.is_none_or(|at| at > Timestamp::now());
        if !claimed {
            // Another worker's claim (MC-23): its token, once it is stored.
            let token = McpToken { bearer: access_token, expired: false };
            return Ok(if fresh { Refresh::Token(token, Box::new(stored.wire)) } else { Refresh::Busy });
        }
        let (Some(refresh), Some(mcp)) = (refresh, &self.mcp) else { return Ok(Refresh::Failed) };
        let Refreshed::Token { access_token, expires_at, refresh_token } =
            request_token(&mcp.guard, &refresh).await
        else {
            self.credentials.refresh_failed(ws, org, &vault, &cred.id).await.map_err(BackendError::new)?;
            return Ok(Refresh::Failed);
        };
        let refresh = OauthRefreshInput {
            refresh_token: refresh_token.unwrap_or(refresh.refresh_token.clone()),
            ..refresh
        };
        let auth = CredentialAuthInput::McpOauth {
            mcp_server_url,
            access_token: access_token.clone(),
            expires_at,
            refresh: Some(refresh),
        };
        let mut wire = stored.wire;
        wire.auth = auth.to_wire();
        wire.updated_at = Timestamp::now();
        let written = StoredCredential { wire: wire.clone(), auth: Some(auth) };
        match self.credentials.finish_refresh(ws, &vault, &written, now).await.map_err(BackendError::new)? {
            RefreshWrite::Matched => {}
            RefreshWrite::Changed => {
                tracing::warn!(credential = %wire.id, "refresh_overwrote_update: an update came during the refresh");
            }
            RefreshWrite::Gone => return Ok(Refresh::Failed),
        }
        Ok(Refresh::Token(McpToken { bearer: access_token, expired: false }, Box::new(wire)))
    }
}

fn server_url(cred: &Credential) -> Option<String> {
    match &cred.auth {
        CredentialAuthWire::McpOauth { mcp_server_url, .. }
        | CredentialAuthWire::StaticBearer { mcp_server_url } => Some(mcp_server_url.clone()),
        CredentialAuthWire::EnvironmentVariable { .. } => None,
    }
}
