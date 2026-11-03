//! Applying an update body to a stored workspace.

use shared::limits::validate_string_len;
use shared::ApiError;

use crate::workspaces::requests::UpdateWorkspaceRequest;
use crate::workspaces::service::{validate_color, validate_residency, validate_tags};
use crate::workspaces::types::Workspace;

impl Workspace {
    /// This workspace with `req` applied, every changed field checked as on
    /// create.
    pub fn updated(mut self, req: UpdateWorkspaceRequest) -> Result<Self, ApiError> {
        if let Some(name) = req.name {
            validate_string_len("name", &name, 1, 255)?;
            self.name = name;
        }
        if let Some(color) = req.display_color {
            validate_color(&color)?;
            self.display_color = color;
        }
        if let Some(key) = req.external_key_id {
            if self.external_key_id.as_ref().is_some_and(|current| *current != key) {
                return Err(ApiError::invalid_request("external_key_id: cannot be changed once set"));
            }
            self.external_key_id = Some(key);
        }
        for (key, value) in req.tags.unwrap_or_default() {
            match value {
                Some(value) => self.tags.insert(key, value),
                None => self.tags.remove(&key),
            };
        }
        validate_tags(&self.tags)?;
        let residency = req.data_residency.unwrap_or_default();
        if let Some(allowed) = residency.allowed_inference_geos {
            self.data_residency.allowed_inference_geos = allowed;
        }
        if let Some(default) = residency.default_inference_geo {
            self.data_residency.default_inference_geo = default;
        }
        validate_residency(&self.data_residency)?;
        Ok(self)
    }
}
