//! Credential auth's wire shape: no secret by construction, so serializing
//! it can never leak one.

use serde::{Deserialize, Serialize};
use shared::Timestamp;

use super::auth::{CredentialNetworking, InjectionLocation, OauthRefreshInput, TokenEndpointAuthInput};

impl OauthRefreshInput {
    pub(super) fn to_wire(&self) -> OauthRefreshWire {
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

/// Credential auth — **wire** shape. Contains no [`SecretString`] by
/// construction, so serializing it can never leak a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CredentialAuthWire {
    /// OAuth (metadata only).
    McpOauth {
        /// Server URL key.
        mcp_server_url: String,
        /// Expiry, when it has one; live writes it to the second, in UTC.
        #[serde(skip_serializing_if = "Option::is_none", serialize_with = "to_the_second")]
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

fn to_the_second<S: serde::Serializer>(at: &Option<Timestamp>, s: S) -> Result<S::Ok, S::Error> {
    match at {
        Some(at) => s.serialize_str(&at.as_datetime().format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        None => s.serialize_none(),
    }
}
