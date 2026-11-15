//! Vault + credential wire types.
//!
//! Live vault fixture:
//! `{archived_at, created_at, display_name, id: vlt_…, metadata, type:"vault", updated_at}`
//!
//! Secrets: input types carry [`SecretString`]; output (wire) types carry
//! only non-secret projections. `SecretString` has no `Serialize` impl —
//! the compiler rejects any attempt to put one on the wire.

use serde::{Deserialize, Serialize};
use shared::{Id, Metadata, Resource, Timestamp};
use zeroize::Zeroizing;

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
/// - `Deserialize` only (secrets flow in, never out),
/// - zeroed when dropped.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    /// Construct from a raw secret.
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[serde(deny_unknown_fields)]
pub struct CreateVaultRequest {
    /// Required.
    pub display_name: Option<String>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}

mod auth;
pub use auth::{
    CredentialAuthInput, CredentialAuthWire, CredentialNetworking, InjectionLocation, OauthRefreshInput,
    TokenEndpointAuthInput,
};

/// Constant `"type":"credential"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialTag {
    /// Always `credential`.
    Credential,
}

/// Wire credential object — secret-free by type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct UpdateCredentialRequest {
    /// Replace display name.
    pub display_name: Option<String>,
    /// Replace auth (must keep the same variant and unique key).
    pub auth: Option<CredentialAuthInput>,
}

/// `POST /v1/vaults/{id}` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateVaultRequest {
    /// Replace display name.
    pub display_name: Option<String>,
    /// Replace metadata.
    pub metadata: Option<Metadata>,
}

#[cfg(test)]
mod tests;
