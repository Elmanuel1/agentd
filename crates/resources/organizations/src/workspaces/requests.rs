//! Workspace request bodies, as `@anthropic-ai/sdk` 1.3.0 types them.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::workspaces::types::{AllowedGeos, InferenceGeo, WorkspaceGeo};

/// `data_residency` on create: each field defaults when left out or `null`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataResidencyCreate {
    /// `"unrestricted"` when left out.
    #[serde(default)]
    pub allowed_inference_geos: Option<AllowedGeos>,
    /// `global` when left out.
    #[serde(default)]
    pub default_inference_geo: Option<InferenceGeo>,
    /// `us` when left out.
    #[serde(default)]
    pub workspace_geo: Option<WorkspaceGeo>,
}

/// `POST /v1/organizations/workspaces` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRequest {
    /// Required, 1 to 255 characters.
    pub name: Option<String>,
    /// The defaults when left out or `null`.
    #[serde(default)]
    pub data_residency: Option<DataResidencyCreate>,
    /// `#RRGGBB`.
    #[serde(default)]
    pub display_color: Option<String>,
    /// The customer-managed encryption key.
    #[serde(default)]
    pub external_key_id: Option<String>,
    /// Labels; no key may start with `anthropic`.
    #[serde(default)]
    pub tags: Option<BTreeMap<String, String>>,
}

/// `data_residency` on update: a field left out or `null` keeps its value.
/// The workspace geo is set at creation only.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataResidencyUpdate {
    /// Replace the allowed geos.
    #[serde(default)]
    pub allowed_inference_geos: Option<AllowedGeos>,
    /// Replace the default geo.
    #[serde(default)]
    pub default_inference_geo: Option<InferenceGeo>,
}

/// `POST /v1/organizations/workspaces/{id}` body: a field left out keeps its
/// value.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceRequest {
    /// 1 to 255 characters.
    #[serde(default)]
    pub name: Option<String>,
    /// `#RRGGBB`.
    #[serde(default)]
    pub display_color: Option<String>,
    /// Set once; the same id again changes nothing.
    #[serde(default)]
    pub external_key_id: Option<String>,
    /// Merged: a string sets the tag, `null` removes it.
    #[serde(default)]
    pub tags: Option<BTreeMap<String, Option<String>>>,
    /// Merged onto the stored residency.
    #[serde(default)]
    pub data_residency: Option<DataResidencyUpdate>,
}
