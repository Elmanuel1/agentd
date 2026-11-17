//! `mcp_oauth_validate`: send `initialize` to the credential's server with
//! its access token, and, when the server refuses it, try the refresh, as
//! live answers (VA-6, B23): `valid` when the server answers, `invalid` when
//! it refuses and no refresh succeeds, `unknown` otherwise.

use async_trait::async_trait;
use serde::Serialize;
use shared::{ApiError, AuthContext, Timestamp};

use crate::mcp::Refresh;
use crate::refresh::HttpResponse;
use crate::service::{VaultsService, map_store, parse_pair};
use crate::store::StoreError;
use crate::types::CredentialAuthInput;

/// What a server answered to `initialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probed {
    /// It answered.
    Answered,
    /// It refused the token (401 or 403).
    Refused(HttpResponse),
    /// Anything else: another status, or no answer at all.
    Failed(Option<HttpResponse>),
}

/// Sends `initialize` to an MCP server: the loop's MCP client.
#[async_trait]
pub trait McpProbe: Send + Sync {
    /// `initialize` to `url` with `bearer`.
    async fn initialize(&self, url: &str, bearer: &str) -> Probed;
}

/// The `mcp_probe` of an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeReport {
    /// The server's answer, when it answered.
    pub http_response: Option<HttpResponse>,
    /// Always `initialize`.
    pub method: &'static str,
}

/// The `refresh` of an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefreshReport {
    /// The token endpoint's refusal, when it refused.
    pub http_response: Option<HttpResponse>,
    /// `succeeded`, `failed` or `no_refresh_token`.
    pub status: &'static str,
}

/// A `vault_credential_validation`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Validation {
    /// The credential.
    pub credential_id: String,
    /// It carries a refresh block.
    pub has_refresh_token: bool,
    /// What the server answered, when it did not accept the token.
    pub mcp_probe: Option<ProbeReport>,
    /// The refresh it led to, when it led to one.
    pub refresh: Option<RefreshReport>,
    /// `valid`, `invalid` or `unknown`.
    pub status: &'static str,
    /// Always `vault_credential_validation`.
    #[serde(rename = "type")]
    pub tag: &'static str,
    /// When.
    pub validated_at: Timestamp,
    /// The credential's vault.
    pub vault_id: String,
}

impl VaultsService {
    /// `POST /v1/vaults/{vid}/credentials/{cid}/mcp_oauth_validate`.
    pub async fn validate_mcp_oauth(
        &self,
        ctx: &AuthContext,
        raw_vault_id: &str,
        raw_cred_id: &str,
    ) -> Result<Validation, ApiError> {
        let (vid, cid) = parse_pair(raw_vault_id, raw_cred_id)?;
        let stored = self.credentials.get_credential(&ctx.workspace, &vid, &cid).await.map_err(map_store)?;
        let Some(CredentialAuthInput::McpOauth { mcp_server_url, access_token, refresh, .. }) =
            stored.auth.clone()
        else {
            return Err(ApiError::invalid_request("Credentials of this auth.type cannot be validated."));
        };
        let probe = self.mcp.as_ref().ok_or_else(|| ApiError::internal("no MCP client"))?;
        let mut v = Validation {
            credential_id: cid.as_str().to_owned(),
            has_refresh_token: refresh.is_some(),
            mcp_probe: None,
            refresh: None,
            status: "valid",
            tag: "vault_credential_validation",
            validated_at: Timestamp::now(),
            vault_id: vid.as_str().to_owned(),
        };
        let report = |http_response| Some(ProbeReport { http_response, method: "initialize" });
        match probe.probe.initialize(&mcp_server_url, access_token.expose()).await {
            Probed::Answered => {}
            Probed::Failed(http_response) => {
                v.status = "unknown";
                v.mcp_probe = report(http_response);
            }
            Probed::Refused(http_response) => {
                v.mcp_probe = report(Some(http_response));
                let (status, outcome) = match refresh {
                    None => ("invalid", RefreshReport { http_response: None, status: "no_refresh_token" }),
                    Some(_) => match self
                        .refresh_mcp_oauth(&ctx.workspace, &ctx.org, stored, false)
                        .await
                        .map_err(|e| map_store(StoreError::Backend(e)))?
                    {
                        Refresh::Token(..) => {
                            ("unknown", RefreshReport { http_response: None, status: "succeeded" })
                        }
                        Refresh::Failed(http_response) => {
                            ("invalid", RefreshReport { http_response, status: "failed" })
                        }
                        Refresh::Busy => ("unknown", RefreshReport { http_response: None, status: "failed" }),
                    },
                };
                v.status = status;
                v.refresh = Some(outcome);
            }
        }
        Ok(v)
    }
}
