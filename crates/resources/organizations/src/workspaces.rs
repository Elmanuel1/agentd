//! Workspaces: `/v1/organizations/workspaces`.

pub mod requests;
pub mod routes;
pub mod service;
pub mod store;
pub mod types;

pub use service::WorkspacesService;
pub use store::{StoreError, WorkspacesStore};
pub use types::{Workspace, WorkspaceId};
