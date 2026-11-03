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

use shared::module::BodyLimit;
use shared::Module;

mod service;

pub use service::{FileFilter, FilesService};

/// The files module: its routes, with the Files API's 500 MB body cap.
pub fn module(service: Arc<FilesService>) -> Module {
    Module::new(routes::router(), service).with_body_limit(BodyLimit { api: "Files", max_mb: 500 })
}
