//! Credential auth: the input shapes, which carry secrets in, and the wire
//! shape, which carries none out.

use serde::{Deserialize, Serialize};
use shared::Timestamp;

use super::SecretString;

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

impl OauthRefreshInput {
    fn to_wire(&self) -> OauthRefreshWire {
        OauthRefreshWire {
            client_id: self.client_id.clone(),
            scope: self.scope.clone(),
            token_endpoint: self.token_endpoint.clone(),
            token_endpoint_auth: self.token_endpoint_auth.as_ref().map(|a| TokenEndpointAuthWire {
                method: match a {
                    TokenEndpointAuthInput::None => TokenEndpointMethod::None,
                    TokenEndpointAuthInput::ClientSecretBasic { .. } => {
                        TokenEndpointMethod::ClientSecretBasic
                    }
                    TokenEndpointAuthInput::ClientSecretPost { .. } => TokenEndpointMethod::ClientSecretPost,
                },
            }),
        }
    }
}

/// OAuth refresh block (wire): everything but the refresh token and the
/// client secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OauthRefreshWire {
    /// OAuth client id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Scope, when one was given.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Where the refresh_token grant goes.
    pub token_endpoint: String,
    /// The endpoint's auth method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth: Option<TokenEndpointAuthWire>,
}

/// A token endpoint's auth method on the wire: its type alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenEndpointAuthWire {
    /// The method.
    #[serde(rename = "type")]
    pub method: TokenEndpointMethod,
}

/// How a client authenticates to its token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEndpointMethod {
    /// Public client.
    None,
    /// HTTP Basic.
    ClientSecretBasic,
    /// In the request body.
    ClientSecretPost,
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
        Self { body: true, header: true }
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
        /// Host restriction. Required by the live API — validated in the service
        /// so the error is the exact `auth.networking: Field required`.
        #[serde(default)]
        networking: Option<CredentialNetworking>,
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
            Self::McpOauth { mcp_server_url, expires_at, refresh, .. } => CredentialAuthWire::McpOauth {
                mcp_server_url: mcp_server_url.clone(),
                expires_at: *expires_at,
                refresh: refresh.as_ref().map(OauthRefreshInput::to_wire),
            },
            Self::StaticBearer { mcp_server_url, .. } => {
                CredentialAuthWire::StaticBearer { mcp_server_url: mcp_server_url.clone() }
            }
            Self::EnvironmentVariable { secret_name, networking, injection_location, .. } => {
                CredentialAuthWire::EnvironmentVariable {
                    secret_name: secret_name.clone(),
                    // Service validated Some before this projection; default is
                    // defensive-only.
                    networking: networking.clone().unwrap_or(CredentialNetworking::Unrestricted),
                    injection_location: injection_location.clone().unwrap_or_default(),
                }
            }
        }
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
        /// Expiry, when it has one.
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<Timestamp>,
        /// How the token refreshes, when it does.
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh: Option<OauthRefreshWire>,
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
