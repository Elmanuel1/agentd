//! engine: the agent loop, run in agentd the way Managed Agents hosts it.
//!
//! - The loop ([`turn`], [`worker`]) runs here and holds the model API keys.
//!   It writes every model and tool step as the session's events, enforces
//!   the agent's permission policies, delegates between session threads and
//!   grades outcomes.
//! - The tools ([`tools`]) run inside the session's sandbox through the
//!   [`shared::ports::sandbox::Sandbox`] port; nothing secret enters it.
//! - [`worker::Dispatcher`] starts a session's loop when client events
//!   arrive, under the session's lease, so one replica runs it at a time.
//!
//! Every non-deterministic dependency is a port: [`model::ModelClient`]
//! (scripted [`model::MockModel`] in tests and CI), the sandbox, the lease
//! and [`worker::SessionAssets`].

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod json;
pub mod model;
pub mod runtime;
pub mod testing;
pub mod tools;
pub mod turn;
pub mod worker;

pub use model::{MockModel, ModelClient, ModelRequest, ModelResponse, ModelRouter};
pub use tools::{SandboxTools, ToolExecutor};
pub use turn::Runner;
pub use worker::{Dispatcher, Engine, SessionAssets};
