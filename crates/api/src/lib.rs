//! api — the composed HTTP surface.
//!
//! This crate holds **no business logic**: it selects a backend for every port
//! from [`config::Config`] ([`backends::Backends`]), builds every entity
//! service, wires the cross-entity dependency ports, assembles each
//! resource's module, and stacks the middleware that makes the wire
//! byte-compatible:
//!
//! 1. **auth** — `x-api-key` → [`AuthContext`] (org, workspace, key id);
//!    missing/unknown → 401 `authentication_error` envelope,
//! 2. **beta gate** — managed-agents routes without
//!    `anthropic-beta: managed-agents-2026-04-01` parse against the non-beta
//!    schema, reproducing the live
//!    `Failed to parse request body: unknown field "…"` 400,
//! 3. **fallback** — unknown routes get the 404 envelope, never a bare body.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod backends;
pub mod config;
pub mod deps;
pub mod keys;
pub mod middleware;
pub mod state;

pub use backends::{Backends, Stores};
pub use config::Config;
pub use keys::EnvKeySource;
pub use state::AppState;

use axum::Router;

/// Build the full agentd router with all middleware.
pub fn build_router(state: AppState) -> Router {
    state
        .modules
        .into_iter()
        .fold(Router::new(), |router, module| router.merge(module.router))
        .fallback(middleware::not_found)
        .layer(axum::middleware::from_fn(middleware::beta_gate))
        .layer(axum::middleware::from_fn_with_state(state.keys, middleware::auth))
}
