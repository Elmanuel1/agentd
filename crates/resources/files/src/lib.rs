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

use async_trait::async_trait;
use shared::module::BodyLimit;
use shared::{ApiError, AuthContext, Module};

mod service;
mod session;

pub use service::{FileFilter, FilesService};

/// What the files API asks of sessions.
#[async_trait]
pub trait SessionFiles: Send + Sync {
    /// Whether `session` reads `file_id`: a file it mounts, or an outcome's rubric.
    async fn reads(&self, ctx: &AuthContext, session: &str, file_id: &str) -> Result<bool, ApiError>;
}

/// The files module: its routes, with the Files API's 500 MB body cap.
pub fn module(service: Arc<FilesService>) -> Module {
    Module::new(routes::router(), service).with_body_limit(BodyLimit { api: "Files", max_mb: 500 })
}
