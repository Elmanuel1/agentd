//! sessions — the Session entity: CRUD, the append-only event log,
//! SSE streaming, the status state machine, resources, and outcome intake.
//!
//! Boundary: this crate is **control-plane data**. The agent loop runs in
//! each session's sandbox, as its harness, and writes into this crate
//! through [`service::HarnessContract`].
//!
//! Live-verified shapes: session object (embeds the full agent snapshot),
//! `sesn_` ids, `status: idle` on create, `outcome_evaluations: []`,
//! `stats {active_seconds, duration_seconds}`, usage block with
//! present-and-null `cache_creation`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod events;
pub mod routes;
pub mod service;
pub mod store;
pub mod types;

use std::sync::Arc;

use shared::beta::BetaGate;
use shared::Module;

pub use events::{Event, EventId, EventPayload, StopReason};
pub use service::{HarnessContract, SandboxWork, SessionDeps, SessionsService, Subscription};
pub use store::{SessionEventLog, SessionEventReads, SessionsStore};
pub use types::{Session, SessionId, SessionStatus};

/// The sessions module: its routes, behind the Managed Agents beta.
pub fn module(service: Arc<SessionsService>) -> Module {
    Module::new(routes::router(), service).with_beta(BetaGate::managed(&["/v1/sessions"]))
}
