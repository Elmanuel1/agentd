//! agentd-api — the composed HTTP surface.
//!
//! This crate holds **no business logic**: it builds every entity service,
//! wires the cross-entity dependency ports, and stacks the middleware that
//! makes the wire byte-compatible:
//!
//! 1. **auth** — `x-api-key` → [`AuthContext`] (org, workspace, key id);
//!    missing/unknown → 401 `authentication_error` envelope,
//! 2. **beta gate** — managed-agents routes without
//!    `anthropic-beta: managed-agents-2026-04-01` parse against the non-beta
//!    schema, reproducing the live
//!    `Failed to parse request body: unknown field "…"` 400,
//! 3. **fallback** — unknown routes get the 404 envelope, never a bare body.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod deps;
pub mod middleware;
pub mod state;

pub use state::{ApiKeyRegistry, AppState};

use axum::Router;

/// Build the full agentd router with all middleware.
pub fn build_router(state: AppState) -> Router {
    let entity_routes = Router::new()
        .merge(agentd_agents::routes::router())
        .merge(agentd_environments::routes::router())
        .merge(agentd_vaults::routes::router())
        .merge(agentd_memory::routes::router())
        .merge(agentd_files::routes::router())
        .merge(agentd_skills::routes::router())
        .merge(agentd_sessions::routes::router())
        .merge(agentd_deployments::routes::router());

    entity_routes
        .fallback(middleware::not_found)
        .layer(axum::middleware::from_fn(middleware::beta_gate))
        .layer(axum::middleware::from_fn_with_state(
            state.keys.clone(),
            middleware::auth,
        ))
        .layer(axum::Extension(state.agents.clone()))
        .layer(axum::Extension(state.environments.clone()))
        .layer(axum::Extension(state.vaults.clone()))
        .layer(axum::Extension(state.memories.clone()))
        .layer(axum::Extension(state.files.clone()))
        .layer(axum::Extension(state.skills.clone()))
        .layer(axum::Extension(state.sessions.clone()))
        .layer(axum::Extension(state.deployments.clone()))
}
