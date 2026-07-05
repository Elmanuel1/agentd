//! Vault + credential wire types.
//!
//! Live vault fixture:
//! `{archived_at, created_at, display_name, id: vlt_…, metadata, type:"vault", updated_at}`
//!
//! Secrets: input types carry [`SecretString`]; output (wire) types carry
//! only non-secret projections. `SecretString` has no `Serialize` impl —
//! the compiler rejects any attempt to put one on the wire.

use agentd_core::{Id, Metadata, Resource, Timestamp};
use serde::{Deserialize, Serialize};

/// Marker for `vlt_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct VaultResource;
impl Resource for VaultResource {
    const PREFIX: &'static str = "vlt";
    const LABEL: &'static str = "vault";
}
/// `Id<VaultResource>` alias.
pub type VaultId = Id<VaultResource>;

/// Marker for `cred_…` ids (prefix unverified against live — flagged for
/// conformance once a credential is created against the real API).
#[derive(Debug, Clone, Copy)]
pub struct CredentialResource;
impl Resource for CredentialResource {
    const PREFIX: &'static str = "cred";
    const LABEL: &'static str = "credential";
}
/// `Id<CredentialResource>` alias.
pub type CredentialId = Id<CredentialResource>;

// ─── SecretString: the non-serializable secret container ──────────────────

/// Secret material. Deliberately:
/// - **no** `Serialize` impl (compile error if placed in a wire type),
/// - redacting `Debug`,
/// - `Deserialize` only (secrets flow in, never out).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    /// Construct from a raw secret.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Expose the secret for egress injection ONLY. Never log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(***)")
    }
}

// ─── Vault ─────────────────────────────────────────────────────────────────

/// Constant `"type":"vault"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultTag {
    /// Always `vault`.
    Vault,
}

/// Full wire vault object (field set matches live probe).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vault {
    /// Archive stamp.
    pub archived_at: Option<Timestamp>,
    /// Created.
    pub created_at: Timestamp,
    /// Display name.
    pub display_name: String,
    /// `vlt_…`.
    pub id: VaultId,
    /// User metadata.
    pub metadata: Metadata,
    /// Always `"vault"`.
    #[serde(rename = "type")]
    pub tag: VaultTag,
    /// Updated.
    pub updated_at: Timestamp,
}

/// `POST /v1/vaults` body.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateVaultRequest {
    /// Required.
    pub display_name: Option<String>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}

// ─── Credential auth: input (secrets in) vs wire (secrets never out) ──────

/// Token-endpoint auth for OAuth refresh.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TokenEndpointAuthInput {
    /// Public client.
    None,
    /// Secret via HTTP Basic.
    ClientSecretBasic {
        /// The client secret (write-only).
        client_secret: SecretString,
    },
    /// Secret in the request body.
    ClientSecretPost {
        /// The client secret (write-only).
        client_secret: SecretString,
    },
}

/// OAuth refresh block (input).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OauthRefreshInput {
    /// Where to POST the refresh_token grant.
    pub token_endpoint: String,
    /// OAuth client id.
    pub client_id: Option<String>,
    /// Scope.
    pub scope: Option<String>,
    /// The refresh token (write-only).
    pub refresh_token: SecretString,
    /// Endpoint auth.
    pub token_endpoint_auth: Option<TokenEndpointAuthInput>,
}

/// Networking restriction for environment-variable credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialNetworking {
    /// Substituted on any outbound host.
    Unrestricted,
    /// Substituted only for these hosts.
    Limited {
        /// Allowed hosts (globs allowed, e.g. `*.twilio.com`).
        allowed_hosts: Vec<String>,
    },
}

/// Where an env-var credential is injected on egress requests. Live default is
/// both `body` and `header` true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionLocation {
    /// Inject into the request body.
    pub body: bool,
    /// Inject into request headers.
    pub header: bool,
}

impl Default for InjectionLocation {
    fn default() -> Self {
        Self {
            body: true,
            header: true,
        }
    }
}

/// Credential auth — **input** shape (secrets flow in).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialAuthInput {
    /// OAuth for an MCP server (auto-refreshed at egress).
    McpOauth {
        /// Which MCP server URL this credential serves (the unique key).
        mcp_server_url: String,
        /// Current access token (write-only).
        access_token: SecretString,
        /// Expiry.
        expires_at: Option<Timestamp>,
        /// Refresh config.
        refresh: Option<OauthRefreshInput>,
    },
    /// Static bearer token for an MCP server.
    StaticBearer {
        /// Which MCP server URL this credential serves (the unique key).
        mcp_server_url: String,
        /// The token (write-only).
        token: SecretString,
    },
    /// Env-var secret substituted at egress.
    EnvironmentVariable {
        /// The variable name (the unique key).
        secret_name: String,
        /// The secret value (write-only).
        secret_value: SecretString,
        /// Host restriction — REQUIRED by the live API.
        networking: CredentialNetworking,
        /// Where to inject; defaults to body + header.
        #[serde(default)]
        injection_location: Option<InjectionLocation>,
    },
}

impl CredentialAuthInput {
    /// The uniqueness key within a vault (`mcp_server_url` or `secret_name`).
    pub fn unique_key(&self) -> &str {
        match self {
            Self::McpOauth { mcp_server_url, .. } | Self::StaticBearer { mcp_server_url, .. } => {
                mcp_server_url
            }
            Self::EnvironmentVariable { secret_name, .. } => secret_name,
        }
    }

    /// The wire projection — provably secret-free (see [`CredentialAuthWire`]).
    pub fn to_wire(&self) -> CredentialAuthWire {
        match self {
            Self::McpOauth {
                mcp_server_url,
                expires_at,
                refresh,
                ..
            } => CredentialAuthWire::McpOauth {
                mcp_server_url: mcp_server_url.clone(),
                expires_at: *expires_at,
                has_refresh: refresh.is_some(),
            },
            Self::StaticBearer { mcp_server_url, .. } => CredentialAuthWire::StaticBearer {
                mcp_server_url: mcp_server_url.clone(),
            },
            Self::EnvironmentVariable {
                secret_name,
                networking,
                injection_location,
                ..
            } => CredentialAuthWire::EnvironmentVariable {
                secret_name: secret_name.clone(),
                networking: networking.clone(),
                injection_location: injection_location.clone().unwrap_or_default(),
            },
        }
    }

    /// Seal the FULL material — **including secrets** — to JSON for a secret
    /// store (S3/MinIO stand-in for a secret manager). This is the deliberate
    /// inverse of [`Self::unseal`]; it is NOT the wire projection and must
    /// never reach a client. The output matches this type's `Deserialize`
    /// shape, so `unseal` is a plain round-trip.
    pub fn seal(&self) -> String {
        match self {
            Self::McpOauth {
                mcp_server_url,
                access_token,
                expires_at,
                refresh,
            } => serde_json::json!({
                "type": "mcp_oauth",
                "mcp_server_url": mcp_server_url,
                "access_token": access_token.expose(),
                "expires_at": expires_at,
                "refresh": refresh.as_ref().map(OauthRefreshInput::seal_value),
            }),
            Self::StaticBearer {
                mcp_server_url,
                token,
            } => serde_json::json!({
                "type": "static_bearer",
                "mcp_server_url": mcp_server_url,
                "token": token.expose(),
            }),
            Self::EnvironmentVariable {
                secret_name,
                secret_value,
                networking,
                injection_location,
            } => serde_json::json!({
                "type": "environment_variable",
                "secret_name": secret_name,
                "secret_value": secret_value.expose(),
                "networking": networking,
                "injection_location": injection_location,
            }),
        }
        .to_string()
    }

    /// Recover sealed material produced by [`Self::seal`].
    pub fn unseal(sealed: &str) -> Option<Self> {
        serde_json::from_str(sealed).ok()
    }
}

impl OauthRefreshInput {
    fn seal_value(&self) -> serde_json::Value {
        let token_endpoint_auth = self.token_endpoint_auth.as_ref().map(|a| match a {
            TokenEndpointAuthInput::None => serde_json::json!({"type": "none"}),
            TokenEndpointAuthInput::ClientSecretBasic { client_secret } => serde_json::json!({
                "type": "client_secret_basic",
                "client_secret": client_secret.expose(),
            }),
            TokenEndpointAuthInput::ClientSecretPost { client_secret } => serde_json::json!({
                "type": "client_secret_post",
                "client_secret": client_secret.expose(),
            }),
        });
        serde_json::json!({
            "token_endpoint": self.token_endpoint,
            "client_id": self.client_id,
            "scope": self.scope,
            "refresh_token": self.refresh_token.expose(),
            "token_endpoint_auth": token_endpoint_auth,
        })
    }
}

/// Credential auth — **wire** shape. Contains no [`SecretString`] by
/// construction, so serializing it can never leak a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialAuthWire {
    /// OAuth (metadata only).
    McpOauth {
        /// Server URL key.
        mcp_server_url: String,
        /// Expiry.
        expires_at: Option<Timestamp>,
        /// Whether auto-refresh is configured.
        has_refresh: bool,
    },
    /// Static bearer (metadata only).
    StaticBearer {
        /// Server URL key.
        mcp_server_url: String,
    },
    /// Env-var secret (metadata only).
    EnvironmentVariable {
        /// Variable name key.
        secret_name: String,
        /// Host restriction (always present on the wire).
        networking: CredentialNetworking,
        /// Where the value is injected on egress.
        injection_location: InjectionLocation,
    },
}

/// Constant `"type":"credential"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialTag {
    /// Always `credential`.
    Credential,
}

/// Wire credential object — secret-free by type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credential {
    /// Archive stamp (archive purges the secret and frees the key).
    pub archived_at: Option<Timestamp>,
    /// Non-secret auth metadata.
    pub auth: CredentialAuthWire,
    /// Created.
    pub created_at: Timestamp,
    /// Display name.
    pub display_name: String,
    /// `cred_…`.
    pub id: CredentialId,
    /// User metadata.
    pub metadata: Metadata,
    /// Always `"credential"`.
    #[serde(rename = "type")]
    pub tag: CredentialTag,
    /// Updated.
    pub updated_at: Timestamp,
    /// Parent vault.
    pub vault_id: String,
}

/// `POST /v1/vaults/{id}/credentials` body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateCredentialRequest {
    /// Required.
    pub display_name: Option<String>,
    /// Required.
    pub auth: Option<CredentialAuthInput>,
    /// Optional user metadata.
    #[serde(default)]
    pub metadata: Metadata,
}

/// `POST /v1/vaults/{vid}/credentials/{cid}` body — rotation only: secret
/// values and display_name can change; keys are immutable.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateCredentialRequest {
    /// Replace display name.
    pub display_name: Option<String>,
    /// Replace auth (must keep the same variant and unique key).
    pub auth: Option<CredentialAuthInput>,
}

/// `POST /v1/vaults/{id}` body.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateVaultRequest {
    /// Replace display name.
    pub display_name: Option<String>,
    /// Replace metadata.
    pub metadata: Option<Metadata>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_string_debug_is_redacted() {
        let s = SecretString::new("xoxp-super-secret");
        assert_eq!(format!("{s:?}"), "SecretString(***)");
        assert_eq!(s.expose(), "xoxp-super-secret");
    }

    #[test]
    fn wire_projection_carries_no_secret_material() {
        let input: CredentialAuthInput = serde_json::from_str(
            r#"{"type":"mcp_oauth","mcp_server_url":"https://mcp.slack.com/mcp",
                "access_token":"xoxp-secret","expires_at":"2026-04-15T00:00:00.000000Z",
                "refresh":{"token_endpoint":"https://slack.com/api/oauth.v2.access",
                           "client_id":"123.456","refresh_token":"xoxe-1-secret",
                           "scope":"channels:read",
                           "token_endpoint_auth":{"type":"client_secret_post","client_secret":"abc"}}}"#,
        )
        .unwrap();
        let wire = input.to_wire();
        let v = serde_json::to_value(&wire).unwrap();
        let raw = v.to_string();
        // No secret substring can appear in the serialized wire form.
        assert!(!raw.contains("xoxp-secret"));
        assert!(!raw.contains("xoxe-1-secret"));
        assert!(!raw.contains("abc"));
        assert_eq!(v["type"], "mcp_oauth");
        assert_eq!(v["mcp_server_url"], "https://mcp.slack.com/mcp");
        assert_eq!(v["has_refresh"], true);
    }

    #[test]
    fn unique_keys_by_variant() {
        let oauth: CredentialAuthInput = serde_json::from_str(
            r#"{"type":"mcp_oauth","mcp_server_url":"https://a/mcp","access_token":"t"}"#,
        )
        .unwrap();
        assert_eq!(oauth.unique_key(), "https://a/mcp");
        let env: CredentialAuthInput = serde_json::from_str(
            r#"{"type":"environment_variable","secret_name":"TWILIO_API_KEY","secret_value":"sk","networking":{"type":"unrestricted"}}"#,
        )
        .unwrap();
        assert_eq!(env.unique_key(), "TWILIO_API_KEY");
    }

    #[test]
    fn env_var_networking_shapes() {
        let n: CredentialNetworking = serde_json::from_str(
            r#"{"type":"limited","allowed_hosts":["api.twilio.com","*.twilio.com"]}"#,
        )
        .unwrap();
        assert!(matches!(n, CredentialNetworking::Limited { .. }));
    }
}
