//! agentd-sessions — the Session entity: CRUD, the append-only event log,
//! SSE streaming, the status state machine, resources, and outcome intake.
//!
//! Boundary: this crate is **control-plane data**. The agent loop itself
//! lives in `agentd-engine` (task-side) and pushes events into this crate
//! through [`service::EngineContract`] — the sessions↔engine seam.
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

pub use events::{Event, EventId, EventPayload, StopReason};
pub use service::{EngineContract, SessionDeps, SessionsService};
pub use store::SessionsStore;
pub use types::{Session, SessionId, SessionStatus};
