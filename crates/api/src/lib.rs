//! api — the composed HTTP surface.
//!
//! This crate holds **no business logic**: it selects a backend for every port
//! from [`config::Config`] ([`backends::Backends`]), builds every entity
//! service, wires the cross-entity dependency ports, assembles each
//! resource's module, and puts the [`edge`] around them.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod assets;
pub mod backends;
pub mod cli;
pub mod config;
pub mod deps;
pub mod edge;
pub mod state;
pub mod telemetry;

pub use backends::{Backends, Stores};
pub use config::Config;
pub use state::AppState;

use std::sync::Arc;

use axum::Router;
use shared::beta::BetaGate;

/// Build the full agentd router: every module's routes, each under its body
/// cap, behind the edge.
pub fn build_router(state: AppState) -> Router {
    let gates: Arc<[BetaGate]> = state.modules.iter().flat_map(|m| m.betas.iter().copied()).collect();
    let routes = state.modules.into_iter().fold(Router::new(), |router, module| {
        router.merge(edge::limits::bounded(module.router, module.body_limit))
    });
    edge::wrap(routes, state.keys, gates, state.metrics)
}
