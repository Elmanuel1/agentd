//! The loop's reads of a session's MCP credentials: which credential each
//! server connects with, and its token, read once in each step that uses it
//! so a rotated, archived or deleted credential applies from the next step.

use std::collections::HashMap;

use shared::{BackendError, OrgId, Timestamp, WorkspaceId};

use crate::refresh::{HttpResponse, Refreshed, request_token};
use crate::service::VaultsService;
use crate::store::{ClaimedWrite, StoreError, StoredCredential};
use crate::types::{
    Credential, CredentialAuthInput, CredentialAuthWire, CredentialId, OauthRefreshInput, SecretString,
    VaultId,
};
use webhooks::EventKind;

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
    /// The refresh failed, with the token endpoint's answer when it gave
    /// one; the server is not called (MC-21, MC-27).
    Failed(Option<HttpResponse>),
}

/// What a server is called with.
#[derive(Debug, Clone)]
pub struct McpToken {
    /// The bearer token.
    pub bearer: SecretString,
    /// An `mcp_oauth` token past its `expires_at`.
    pub expired: bool,
    /// The secret it was read from, which a refresh starts from.
    pub secret: CredentialAuthInput,
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
            let url = match &cred.auth {
                CredentialAuthWire::McpOauth { mcp_server_url, .. }
                | CredentialAuthWire::StaticBearer { mcp_server_url } => mcp_server_url.clone(),
                CredentialAuthWire::EnvironmentVariable { .. } => continue,
            };
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
            Ok(secret) => Ok(token_of(secret)),
            Err(StoreError::CredentialNotFound) => Ok(None),
            Err(e) => Err(BackendError::new(e)),
        }
    }
}

impl VaultsService {
    /// Refresh `stored`'s token (the row and secret the caller read),
    /// claimed by moving the row's `updated_at` from what was read, so one
    /// writer refreshes (MC-7). With `hold`, a row updated in the last 30 s
    /// is another worker's refresh in flight (MC-20).
    pub async fn refresh_mcp_oauth(
        &self,
        ws: &WorkspaceId,
        org: &OrgId,
        stored: StoredCredential,
        hold: bool,
    ) -> Result<Refresh, BackendError> {
        let now = Timestamp::now();
        let read = stored.wire.updated_at;
        if hold && now.as_datetime() - read.as_datetime() < chrono::Duration::seconds(CLAIM_HOLDS) {
            return Ok(Refresh::Busy);
        }
        let vault = VaultId::parse(&stored.wire.vault_id).map_err(BackendError::new)?;
        let id = stored.wire.id.clone();
        if !self.credentials.claim_credential(ws, &vault, &id, read, now).await.map_err(BackendError::new)? {
            return self.claimed_elsewhere(ws, &vault, &id).await;
        }
        let Some(CredentialAuthInput::McpOauth { mcp_server_url, refresh: Some(refresh), .. }) =
            stored.auth.clone()
        else {
            return Ok(Refresh::Failed(None));
        };
        let Some(mcp) = &self.mcp else { return Ok(Refresh::Failed(None)) };
        let (access_token, expires_at, rotated) = match request_token(&mcp.guard, &refresh).await {
            Refreshed::Token { access_token, expires_at, refresh_token } => {
                (access_token, expires_at, refresh_token)
            }
            Refreshed::Failed(answer) => {
                let kind = EventKind::CredentialRefreshFailed { vault_id: vault.as_str().to_owned() };
                self.credentials
                    .insert_deliveries(&self.deliveries(org, ws, kind, id.as_str()))
                    .await
                    .map_err(BackendError::new)?;
                return Ok(Refresh::Failed(answer));
            }
        };
        let refresh =
            OauthRefreshInput { refresh_token: rotated.unwrap_or(refresh.refresh_token.clone()), ..refresh };
        let auth = CredentialAuthInput::McpOauth {
            mcp_server_url,
            access_token: access_token.clone(),
            expires_at,
            refresh: Some(refresh),
        };
        let mut written = stored;
        written.wire.auth = auth.to_wire();
        written.wire.updated_at = Timestamp::now();
        written.auth = Some(auth.clone());
        match self.credentials.write_claimed(ws, &vault, &written, now).await.map_err(BackendError::new)? {
            ClaimedWrite::Matched => {}
            ClaimedWrite::Changed => {
                tracing::warn!(credential = %id, "refresh_overwrote_update: an update came during the refresh");
            }
            ClaimedWrite::Gone => return Ok(Refresh::Failed(None)),
        }
        let token = McpToken { bearer: access_token, expired: false, secret: auth };
        Ok(Refresh::Token(token, Box::new(written.wire)))
    }

    /// Another writer's claim won (MC-23): its token once it is stored and
    /// fresh, else the step waits.
    async fn claimed_elsewhere(
        &self,
        ws: &WorkspaceId,
        vault: &VaultId,
        id: &CredentialId,
    ) -> Result<Refresh, BackendError> {
        let stored = match self.credentials.get_credential(ws, vault, id).await {
            Ok(stored) => stored,
            Err(StoreError::CredentialNotFound) => return Ok(Refresh::Failed(None)),
            Err(e) => return Err(BackendError::new(e)),
        };
        let Some(token) = stored.auth.and_then(token_of) else { return Ok(Refresh::Failed(None)) };
        if token.expired {
            return Ok(Refresh::Busy);
        }
        Ok(Refresh::Token(token, Box::new(stored.wire)))
    }
}

/// What `secret` calls a server with; `None` for a secret no server takes.
fn token_of(secret: CredentialAuthInput) -> Option<McpToken> {
    let (bearer, expired) = match &secret {
        CredentialAuthInput::McpOauth { access_token, expires_at, .. } => {
            (access_token.clone(), expires_at.is_some_and(|at| at <= Timestamp::now()))
        }
        CredentialAuthInput::StaticBearer { token, .. } => (token.clone(), false),
        CredentialAuthInput::EnvironmentVariable { .. } => return None,
    };
    Some(McpToken { bearer, expired, secret })
}
