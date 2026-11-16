//! A credential update's body, as live takes it: per auth type, only the
//! fields that rotate; anything else is an unknown field. A field left out
//! or sent as `null` keeps its value.

use serde::Deserialize;
use shared::patch::{MetadataPatch, Patch};
use shared::Timestamp;

use super::auth::{CredentialNetworking, InjectionLocation};
use super::SecretString;

/// `POST /v1/vaults/{vid}/credentials/{cid}` body.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateCredentialRequest {
    /// A new display name.
    pub display_name: Option<String>,
    /// The fields of the credential's auth that rotate.
    pub auth: Option<CredentialAuthUpdate>,
    /// Keys to set, and keys sent as `null` to remove.
    #[serde(default)]
    pub metadata: Patch<MetadataPatch>,
}

/// The rotating fields of each auth type.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialAuthUpdate {
    /// An OAuth credential: its token, expiry and refresh secrets.
    McpOauth {
        /// A new access token; without `expires_at` the stored expiry stays.
        access_token: Option<SecretString>,
        /// A new expiry.
        expires_at: Option<Timestamp>,
        /// The refresh's rotating fields.
        refresh: Option<RefreshUpdate>,
    },
    /// A static bearer: its token.
    StaticBearer {
        /// A new token.
        token: Option<SecretString>,
    },
    /// An environment variable: its value and where it may go.
    EnvironmentVariable {
        /// A new value.
        secret_value: Option<SecretString>,
        /// New host limits.
        networking: Option<CredentialNetworking>,
        /// New injection places.
        injection_location: Option<InjectionLocation>,
    },
}

/// A refresh's rotating fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefreshUpdate {
    /// A new refresh token.
    pub refresh_token: Option<SecretString>,
    /// A new scope.
    pub scope: Option<String>,
    /// A new client secret, under the stored method.
    pub token_endpoint_auth: Option<ClientSecretUpdate>,
}

/// A new client secret. Its `type` names a secret-bearing method; the stored
/// method stays whichever is sent.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSecretUpdate {
    /// `client_secret_basic` or `client_secret_post`; `none` is refused.
    #[serde(rename = "type")]
    pub method: String,
    /// The new secret.
    pub client_secret: Option<SecretString>,
}
