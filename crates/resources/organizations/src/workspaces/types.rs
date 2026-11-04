//! The workspace wire object, as `@anthropic-ai/sdk` 1.3.0 types it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shared::{Id, Resource, Timestamp};

/// Marker for `wrkspc_…` ids.
#[derive(Debug, Clone, Copy)]
pub struct WorkspaceResource;
impl Resource for WorkspaceResource {
    const PREFIX: &'static str = "wrkspc";
    const LABEL: &'static str = "workspace";
}
/// `Id<WorkspaceResource>` alias.
pub type WorkspaceId = Id<WorkspaceResource>;

/// Constant `"type":"workspace"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceTag {
    /// Always `workspace`.
    Workspace,
}

/// Where inference may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceGeo {
    /// Anywhere.
    Global,
    /// The United States.
    Us,
}

/// Where a workspace's data lives; `us` is the only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceGeo {
    /// The United States.
    Us,
}

/// The `"unrestricted"` keyword of `allowed_inference_geos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unrestricted {
    /// Every geo.
    Unrestricted,
}

/// `allowed_inference_geos`: a list of geos, or `"unrestricted"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AllowedGeos {
    /// Every geo.
    Unrestricted(Unrestricted),
    /// Only these.
    Only(Vec<InferenceGeo>),
}

impl AllowedGeos {
    /// Whether inference may run in `geo`.
    #[must_use]
    pub fn allows(&self, geo: InferenceGeo) -> bool {
        match self {
            Self::Unrestricted(_) => true,
            Self::Only(geos) => geos.contains(&geo),
        }
    }
}

/// A workspace's data residency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataResidency {
    /// Where inference may run.
    pub allowed_inference_geos: AllowedGeos,
    /// Where inference runs when a request names no geo.
    pub default_inference_geo: InferenceGeo,
    /// Where the workspace's data lives; set at creation.
    pub workspace_geo: WorkspaceGeo,
}

impl Default for DataResidency {
    fn default() -> Self {
        Self {
            allowed_inference_geos: AllowedGeos::Unrestricted(Unrestricted::Unrestricted),
            default_inference_geo: InferenceGeo::Global,
            workspace_geo: WorkspaceGeo::Us,
        }
    }
}

/// A workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// `wrkspc_…`.
    pub id: String,
    /// Always `workspace`.
    #[serde(rename = "type")]
    pub tag: WorkspaceTag,
    /// 1 to 255 characters.
    pub name: String,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it was archived; `null` while active.
    pub archived_at: Option<Timestamp>,
    /// `#RRGGBB`.
    pub display_color: String,
    /// Where its data lives and its inference runs.
    pub data_residency: DataResidency,
    /// Labels; no key starts with `anthropic`.
    pub tags: BTreeMap<String, String>,
    /// The customer-managed encryption key, when one is set.
    pub external_key_id: Option<String>,
    /// The encryption compartment.
    pub compartment_id: String,
}
