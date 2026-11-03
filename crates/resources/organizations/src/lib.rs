//! organizations: the Admin API, `/v1/organizations`. Every route here is
//! reached with an admin key only, which the auth edge resolves to an
//! [`shared::AdminContext`].
//!
//! agentd keeps no organization record: an organization is the id its keys
//! carry, so `GET /v1/organizations/me` names the organization by that id.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;
pub mod types;
pub mod workspaces;

use std::sync::Arc;

use shared::Module;

pub use types::Organization;
pub use workspaces::WorkspacesService;

/// The organizations module. The Admin API sits behind no beta.
pub fn module(workspaces: Arc<WorkspacesService>) -> Module {
    Module::new(routes::router().merge(workspaces::routes::router()), workspaces)
}
