//! organizations: the Admin API, `/v1/organizations`. Every route here is
//! reached with an admin key only, which the auth edge resolves to an
//! [`shared::AdminContext`].
//!
//! An organization is one `organizations` row ([`registry`]), made and deleted with the `agentd
//! organization` command; `GET /v1/organizations/me` names it by its id.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod deletion;
pub mod registry;
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
