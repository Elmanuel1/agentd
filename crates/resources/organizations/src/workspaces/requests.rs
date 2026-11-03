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
