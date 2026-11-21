//! files — the Files support resource (`/v1/files`).
//!
//! Upload via multipart, list (with `scope_id` filter for session outputs),
//! metadata get, raw download, delete. Delete-only lifecycle (no archive).
//! Wire shapes follow the documented Files API; conformance against live
//! uploads is verified by the harness (marked: shape unprobed live).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;
pub mod store;
pub mod types;

use std::sync::Arc;

use shared::Module;
use shared::module::BodyLimit;

mod service;
mod session;

pub use service::{FileFilter, FilesService};
pub use session::{Mounts, NativeSessionFiles, SessionFiles};

/// The files module: its routes, with the Files API's 500 MB body cap;
/// `sessions` says which files a session's work token reaches.
pub fn module(service: Arc<FilesService>, sessions: Arc<dyn SessionFiles>) -> Module {
    let router = routes::router().layer(axum::Extension(Mounts(sessions)));
    // 500 MB at about 5 Mbit/s.
    Module::new(router, service)
        .with_body_limit(BodyLimit { api: "Files", max_mb: 500 })
        .with_deadline(std::time::Duration::from_secs(900))
}
