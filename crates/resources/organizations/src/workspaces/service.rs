//! Workspace business logic. Workspaces are org-scoped: every operation reads
//! and writes the admin key's organization only.

use std::collections::BTreeMap;
use std::sync::Arc;

use shared::limits::{required, validate_string_len};
use shared::{AdminContext, ApiError, IdCursor, Timestamp};

use crate::workspaces::requests::{CreateWorkspaceRequest, DataResidencyCreate};
use crate::workspaces::store::{StoreError, WorkspacesStore};
use crate::workspaces::types::{DataResidency, Workspace, WorkspaceId, WorkspaceTag};

/// The colour a workspace created without one gets.
pub const DEFAULT_COLOR: &str = "#6c5bb9";

/// Workspace service.
pub struct WorkspacesService {
    store: Arc<dyn WorkspacesStore>,
}

impl std::fmt::Debug for WorkspacesService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspacesService").finish_non_exhaustive()
    }
}

fn store_error(e: StoreError) -> ApiError {
    match e {
        StoreError::NotFound => ApiError::not_found("workspace not found"),
        StoreError::Backend(e) => ApiError::internal(e.to_string()),
    }
}

/// `#` and six hex digits.
pub fn validate_color(color: &str) -> Result<(), ApiError> {
    let hex = color.strip_prefix('#').filter(|h| h.len() == 6 && h.bytes().all(|b| b.is_ascii_hexdigit()));
    hex.map(|_| ()).ok_or_else(|| {
        ApiError::invalid_request("display_color: String should match pattern '^#[0-9A-Fa-f]{6}$'")
    })
}

/// No tag key may start with `anthropic`.
pub fn validate_tags(tags: &BTreeMap<String, String>) -> Result<(), ApiError> {
    match tags.keys().find(|k| k.starts_with("anthropic")) {
        Some(key) => {
            Err(ApiError::invalid_request(format!("tags: key `{key}` may not begin with `anthropic`")))
        }
        None => Ok(()),
    }
}

/// The default geo must be one the workspace allows.
pub fn validate_residency(residency: &DataResidency) -> Result<(), ApiError> {
    if residency.allowed_inference_geos.allows(residency.default_inference_geo) {
        return Ok(());
    }
    Err(ApiError::invalid_request(
        "data_residency.default_inference_geo: must be one of data_residency.allowed_inference_geos",
    ))
}

fn residency(params: DataResidencyCreate) -> DataResidency {
    let defaults = DataResidency::default();
    DataResidency {
        allowed_inference_geos: params.allowed_inference_geos.unwrap_or(defaults.allowed_inference_geos),
        default_inference_geo: params.default_inference_geo.unwrap_or(defaults.default_inference_geo),
        workspace_geo: params.workspace_geo.unwrap_or(defaults.workspace_geo),
    }
}

impl WorkspacesService {
    /// Build with a store.
    pub fn new(store: Arc<dyn WorkspacesStore>) -> Self {
        Self { store }
    }

    /// `POST /v1/organizations/workspaces`.
    pub async fn create(
        &self,
        ctx: &AdminContext,
        req: CreateWorkspaceRequest,
    ) -> Result<Workspace, ApiError> {
        let name = required("name", req.name)?;
        validate_string_len("name", &name, 1, 255)?;
        let display_color = req.display_color.unwrap_or_else(|| DEFAULT_COLOR.to_owned());
        validate_color(&display_color)?;
        let tags = req.tags.unwrap_or_default();
        validate_tags(&tags)?;
        let data_residency = residency(req.data_residency.unwrap_or_default());
        validate_residency(&data_residency)?;
        let id = WorkspaceId::generate().to_string();
        let workspace = Workspace {
            compartment_id: id.clone(),
            id,
            tag: WorkspaceTag::Workspace,
            name,
            created_at: Timestamp::now(),
            archived_at: None,
            display_color,
            data_residency,
            tags,
            external_key_id: req.external_key_id,
        };
        self.store.insert(&ctx.org, &workspace).await.map_err(store_error)?;
        Ok(workspace)
    }

    /// `GET /v1/organizations/workspaces/{id}`.
    pub async fn get(&self, ctx: &AdminContext, id: &str) -> Result<Workspace, ApiError> {
        self.store.get(&ctx.org, id).await.map_err(store_error)
    }

    /// `GET /v1/organizations/workspaces`: one page and whether more follow.
    pub async fn list(
        &self,
        ctx: &AdminContext,
        include_archived: bool,
        cursor: &IdCursor,
    ) -> Result<(Vec<Workspace>, bool), ApiError> {
        self.store.list(&ctx.org, include_archived, cursor).await.map_err(store_error)
    }
}

#[cfg(test)]
mod tests;
