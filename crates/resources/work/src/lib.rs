//! work: the environments work API. Each session has one work item, its id
//! the session's, kept in Postgres. A worker's poller takes it from its
//! queue with an environment key and the worker serves the session with the
//! item's work token; a hosted session's own sandbox takes its item itself.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod caller;
pub mod lifecycle;
pub mod placeholder;
pub mod routes;
pub mod secret;
pub mod service;
pub mod store;
pub mod tokens;
pub mod types;

use std::sync::Arc;

use shared::beta::BetaGate;
use shared::Module;

pub use caller::WorkCaller;
pub use placeholder::{PlaceholderClaims, Placeholders};
pub use secret::{CloudVariables, SessionVariables, Variable};
pub use service::WorkService;
pub use store::{WorkItem, WorkReads, WorkStore, HOSTED, RUNTIME};
pub use tokens::{WorkClaims, WorkTokens};
pub use types::{Work, WorkState};

/// The work module: its routes, behind the Managed Agents beta.
pub fn module(service: Arc<WorkService>) -> Module {
    Module::new(routes::router(), service).with_beta(BetaGate::managed(&["/v1/environments"]))
}
