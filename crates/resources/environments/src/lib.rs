//! environments — the Environment entity.
//!
//! The only **organization-scoped** resource (live-verified): environments
//! are shared across every workspace in the org, and the wire carries
//! `"scope":"organization"` — on this resource only.
//!
//! Live-verified divergences from docs:
//! - Duplicate environment names are **allowed** (dup create → 200, new id).
//! - `DELETE` returned 200 (docs claim 204).
//! - `description` canonicalizes to `""` (empty string), not null.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod routes;
pub mod service;
pub mod store;
pub mod types;

use std::sync::Arc;

use shared::beta::BetaGate;
use shared::Module;

pub use service::EnvironmentsService;
pub use store::EnvironmentsStore;
pub use types::{Environment, EnvironmentConfig, EnvironmentId};

/// The environments module: its routes, behind the Managed Agents beta.
pub fn module(service: Arc<EnvironmentsService>) -> Module {
    Module::new(routes::router(), service).with_beta(BetaGate::managed(&["/v1/environments"]))
}
