//! Workspaces: `/v1/organizations/workspaces`.

pub mod store;
pub mod types;

pub use store::{StoreError, WorkspacesStore};
pub use types::{Workspace, WorkspaceId};
