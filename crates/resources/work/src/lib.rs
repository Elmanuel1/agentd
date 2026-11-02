//! work: the environments work API. Each hosted session has a work item;
//! the worker in its sandbox acknowledges it, keeps its lease with
//! heartbeats, and long-polls for the sandbox operations the agent loop
//! queues ([`RemoteSandbox`]). Self-hosted workers poll an environment for
//! queued work the same way.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod board;
mod lifecycle;
pub mod routes;
mod sandbox;
pub mod service;
pub mod tokens;
pub mod types;

use std::sync::Arc;

use shared::beta::BetaGate;
use shared::Module;

pub use sandbox::RemoteSandbox;
pub use service::{WorkScope, WorkService};
pub use tokens::{WorkClaims, WorkTokens};
pub use types::{Work, WorkId, WorkState};

/// The work module: its routes, behind the Managed Agents beta.
pub fn module(service: Arc<WorkService>) -> Module {
    Module::new(routes::router(), service).with_beta(BetaGate::managed(&["/v1/environments"]))
}

#[cfg(test)]
mod tests;
