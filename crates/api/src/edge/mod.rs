//! The HTTP edge every route sits behind, matching the live API:
//!
//! 1. [`request_id`]: a `request-id` on every response, the same id as the
//!    error envelope's `request_id`,
//! 2. [`auth`]: `x-api-key` or `Authorization: Bearer` → [`shared::AuthContext`],
//! 3. [`contract`]: `anthropic-version` is required, every `anthropic-beta`
//!    name must be known, and a resource's beta-gated routes read a request
//!    without their beta against the GA schema,
//! 4. [`methods`]: HEAD and unserved methods are 405s with the live body; an
//!    unknown path is the live 404 (`request_id: null`, live auth/08),
//! 5. [`limits`]: each module's body cap, a 413 above it.
//!
//! Routing comes first, as live: an unknown path is a 404 before any auth.

pub mod auth;
pub mod contract;
pub mod limits;
pub mod methods;
pub mod request_id;

use std::sync::Arc;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::Router;
use shared::beta::BetaGate;
use shared::ports::keys::KeySource;

/// Put the edge around the merged module routes.
pub fn wrap(routes: Router, keys: Arc<dyn KeySource>, gates: Arc<[BetaGate]>) -> Router {
    routes
        .method_not_allowed_fallback(methods::not_allowed)
        .route_layer(from_fn_with_state(gates, contract::betas))
        .route_layer(from_fn(contract::version))
        .route_layer(from_fn(methods::reject_head))
        .route_layer(from_fn_with_state(keys, auth::auth))
        .fallback(|| async { shared::unknown_route_response() })
        .layer(from_fn(request_id::request_id))
}
