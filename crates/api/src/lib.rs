//! api — the composed HTTP surface.
//!
//! This crate holds **no business logic**: it selects a backend for every port
//! from [`config::Config`] ([`backends::Backends`]), builds every entity
//! service, wires the cross-entity dependency ports, assembles each
//! resource's module, and puts the [`edge`] around them.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod agent_loop;
pub mod backends;
pub mod cli;
pub mod config;
pub mod deps;
pub mod edge;
pub mod session_grants;
pub mod session_work;
pub mod shutdown;
pub mod state;
pub mod telemetry;

pub use backends::{Backends, Stores, SystemKeys};
pub use config::Config;
pub use state::AppState;

use std::sync::Arc;

use axum::Router;
use shared::beta::BetaGate;

/// Build the full agentd router: every module's routes, each under its body
/// cap, behind the edge.
pub fn build_router(state: AppState) -> Router {
    if state.modules.is_empty() {
        // A `loop` replica; axum refuses route layers on a router with no route.
        return Router::new().fallback(|| async { shared::unknown_route_response() });
    }
    let gates: Arc<[BetaGate]> = state.modules.iter().flat_map(|m| m.betas.iter().copied()).collect();
    let (timeout, limits) = (state.edge.request_timeout, state.edge.limits);
    let routes = state.modules.into_iter().fold(Router::new(), |router, module| {
        let bounded = edge::limits::bounded(module.router, module.body_limit);
        router.merge(edge::deadline::bounded(bounded, module.deadline.unwrap_or(timeout)))
    });
    let credentials = edge::auth::Credentials {
        keys: state.keys,
        admin_keys: state.admin_keys,
        environment_keys: state.environment_keys,
        work: state.work,
    };
    edge::wrap(routes, credentials, gates, state.metrics, limits)
}
