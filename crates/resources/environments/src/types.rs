//! Environment wire types — canonical shapes from the live fixture.
//!
//! Input `{"type":"cloud","networking":{"type":"unrestricted"}}` expands to:
//! ```json
//! {"type":"cloud",
//!  "packages":{"type":"packages","pip":[],"npm":[],"apt":[],"cargo":[],"gem":[],"go":[]},
//!  "networking":{"type":"unrestricted"},
//!  "init_script":"", "environment":{}}
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shared::{Id, Metadata, Resource, Scope, Timestamp};

/// Marker for `env_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct EnvironmentResource;
impl Resource for EnvironmentResource {
    const PREFIX: &'static str = "env";
    const LABEL: &'static str = "environment";
}
/// `Id<EnvironmentResource>` alias.
pub type EnvironmentId = Id<EnvironmentResource>;

/// Constant `"type":"environment"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentTag {
    /// Always `environment`.
    Environment,
}

/// Environment lifecycle state on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentState {
    /// Usable by new sessions.
    Active,
    /// Read-only (archived).
    Archived,
}

/// Package sets preinstalled into containers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Packages {
    /// Always `"packages"`.
    #[serde(rename = "type")]
    pub tag: PackagesTag,
    /// pip packages.
    #[serde(default)]
    pub pip: Vec<String>,
    /// npm packages.
    #[serde(default)]
    pub npm: Vec<String>,
    /// apt packages.
    #[serde(default)]
    pub apt: Vec<String>,
    /// cargo crates.
    #[serde(default)]
    pub cargo: Vec<String>,
    /// gems.
    #[serde(default)]
    pub gem: Vec<String>,
    /// go modules.
    #[serde(default)]
    pub go: Vec<String>,
}

/// Constant `"type":"packages"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackagesTag {
    /// Always `packages`.
    Packages,
}

impl Default for Packages {
    fn default() -> Self {
        Self {
            tag: PackagesTag::Packages,
            pip: vec![],
            npm: vec![],
            apt: vec![],
            cargo: vec![],
            gem: vec![],
            go: vec![],
        }
    }
}

/// Container networking policy.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Networking {
    /// Full egress.
    #[default]
    Unrestricted,
    /// Deny-by-default with opt-ins.
    Limited {
        /// Allowed egress hosts.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        allowed_hosts: Vec<String>,
        /// Permit package managers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_package_managers: Option<bool>,
        /// Permit configured MCP server endpoints.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_mcp_servers: Option<bool>,
    },
}

/// Canonical environment config.
// Boxing the `Cloud` variant would change the internally-tagged serde shape and
// break wire conformance; the size asymmetry is harmless for a config object
// that is never held in bulk.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EnvironmentConfig {
    /// Anthropic-hosted (here: agentd-hosted) containers.
    Cloud {
        /// Preinstalled packages (canonical: always present).
        #[serde(default)]
        packages: Packages,
        /// Networking policy (canonical: always present).
        #[serde(default)]
        networking: Networking,
        /// Startup script (canonical: `""`).
        #[serde(default)]
        init_script: String,
        /// Extra env vars (canonical: `{}`).
        #[serde(default)]
        environment: BTreeMap<String, String>,
    },
    /// Tool execution on customer infrastructure. Bare on the wire.
    SelfHosted {},
}

/// Full wire environment object (field order matches the live fixture).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    /// `env_…`.
    pub id: EnvironmentId,
    /// Always `"environment"`.
    #[serde(rename = "type")]
    pub tag: EnvironmentTag,
    /// Name — **not unique** (live-verified).
    pub name: String,
    /// Description — canonical default `""` (live-verified, not null).
    pub description: String,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
    /// Archive stamp.
    pub archived_at: Option<Timestamp>,
    /// Lifecycle state.
    pub state: EnvironmentState,
    /// Canonical config.
    pub config: EnvironmentConfig,
    /// User metadata.
    pub metadata: Metadata,
    /// Always `"organization"` — the only resource that carries scope.
    pub scope: Scope,
}

/// `POST /v1/environments` body (permissive; validated in the service).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvironmentRequest {
    /// Required.
    pub name: Option<String>,
    /// Optional (canonicalizes to `""`).
    pub description: Option<String>,
    /// Required.
    pub config: Option<EnvironmentConfig>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
}

/// `POST /v1/environments/{id}` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateEnvironmentRequest {
    /// Replace name.
    pub name: Option<String>,
    /// Replace description.
    pub description: Option<String>,
    /// Replace config (applies to **new** containers only).
    pub config: Option<EnvironmentConfig>,
    /// Replace metadata.
    pub metadata: Option<Metadata>,
}

#[cfg(test)]
mod tests;
