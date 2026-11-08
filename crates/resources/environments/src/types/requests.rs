use serde::Deserialize;
use shared::{Metadata, MetadataPatch, Patch};

use super::ConfigParams;

/// The requested visibility. API organizations support only `organization`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeParam {
    /// Visible to the whole organization.
    Organization,
    /// Visible to the owning account only: rejected for API organizations.
    Account,
}

/// `POST /v1/environments` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateEnvironmentRequest {
    /// Required.
    pub name: Option<String>,
    /// `""` when left out; `null` stores null.
    #[serde(default)]
    pub description: Patch<String>,
    /// A cloud config with unrestricted networking when left out or `null`.
    #[serde(default)]
    pub config: Option<ConfigParams>,
    /// Optional.
    #[serde(default)]
    pub metadata: Metadata,
    /// `organization` when left out or `null`.
    #[serde(default)]
    pub scope: Option<ScopeParam>,
}

/// `POST /v1/environments/{id}` body: a field left out keeps its value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateEnvironmentRequest {
    /// Replace the name (`null` keeps it).
    #[serde(default)]
    pub name: Option<String>,
    /// Replace, or clear to `null`.
    #[serde(default)]
    pub description: Patch<String>,
    /// Merge onto the stored config (`null` keeps it).
    #[serde(default)]
    pub config: Option<ConfigParams>,
    /// Merge: strings upsert, `null` or `""` removes.
    #[serde(default)]
    pub metadata: Patch<MetadataPatch>,
    /// `organization` (`null` keeps it).
    #[serde(default)]
    pub scope: Option<ScopeParam>,
}
