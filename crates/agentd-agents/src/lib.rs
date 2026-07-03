//! agentd-agents — the Agent entity (persisted, integer-versioned).
//!
//! Vertical slice: wire types, store, service (validation + canonicalization
//! with exact live error strings), and axum routes. Other entities embed
//! [`types::AgentSnapshot`] (sessions/threads) or [`types::AgentRef`]
//! (deployments) — both exported here.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod routes;
pub mod service;
pub mod store;
pub mod types;

pub use service::AgentsService;
pub use store::{AgentsStore, InMemoryAgentsStore};
pub use types::{Agent, AgentConfig, AgentRef, AgentSnapshot, ModelSpec};
