//! The HTTP edge every route sits behind, matching the live API:
//!
//! 1. [`request_id`]: a `request-id` on every response, the same id as the
//!    error envelope's `request_id`,
//! 2. [`auth`]: `x-api-key` or `Authorization: Bearer` → [`shared::AuthContext`],
//!    then [`rate`]: the organization's requests counted per bucket, a 429 past its limit,
//! 3. [`contract`]: `anthropic-version` is required, every `anthropic-beta`
//!    name must be known, and a resource's beta-gated routes read a request
//!    without their beta against the GA schema,
//! 4. [`methods`]: HEAD and unserved methods are 405s with the live body; an
//!    unknown path is the live 404 (`request_id: null`, live auth/08),
//! 5. [`limits`]: each module's body cap, a 413 above it, and [`deadline`]:
//!    each module's request deadline, a 504 past it, and around auth and
//!    every route a 529 for a request whose database checkout timed out,
//! 6. [`telemetry::http`]: a span and a duration sample per request, inside
//!    the request id so both carry it.
//!
//! Routing comes first, as live: an unknown path is a 404 before any auth.

pub mod auth;
pub mod contract;
pub mod deadline;
pub mod limits;
pub mod methods;
pub mod rate;
pub mod reach;
pub mod request_id;
pub mod scoped;

use std::sync::Arc;

use axum::Router;
use axum::middleware::{from_fn, from_fn_with_state};
use shared::beta::BetaGate;

use crate::telemetry::{self, Metrics};

/// What the edge applies to every module, from the deployment's settings.
#[derive(Debug, Clone)]
pub struct EdgeSettings {
    /// `AGENTD_REQUEST_TIMEOUT_MS` (60,000): how long a response may take to start.
    pub request_timeout: std::time::Duration,
    /// Where requests are counted against live's limits; none counts nothing.
    pub limits: Option<rate::Limits>,
}

impl Default for EdgeSettings {
    fn default() -> Self {
        Self { request_timeout: std::time::Duration::from_secs(60), limits: None }
    }
}

/// Put the edge around the merged module routes.
pub fn wrap(
    routes: Router,
    credentials: auth::Credentials,
    gates: Arc<[BetaGate]>,
    metrics: Arc<Metrics>,
    limits: Option<rate::Limits>,
) -> Router {
    let routes = match limits {
        Some(limits) => routes.route_layer(from_fn_with_state(limits, rate::limit)),
        None => routes,
    };
    routes
        .method_not_allowed_fallback(methods::not_allowed)
        .route_layer(from_fn_with_state(gates, contract::betas))
        .route_layer(from_fn(contract::version))
        .route_layer(from_fn(methods::reject_head))
        .route_layer(from_fn_with_state(credentials, auth::auth))
        .route_layer(from_fn(deadline::shed))
        .fallback(|| async { shared::unknown_route_response() })
        .layer(from_fn_with_state(metrics, telemetry::http::observe))
        .layer(from_fn(request_id::request_id))
}
