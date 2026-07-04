//! agentd-engine — the task-side agent loop.
//!
//! Runs inside the ephemeral job (Fargate task / Firecracker VM / local
//! tokio task) and drives one session: drain queued user events → call the
//! model → execute tools under the agent's permission policy → emit the
//! wire events through [`agentd_sessions::EngineContract`] → idle.
//!
//! Every non-deterministic dependency is a port:
//! - [`model::ModelClient`] — the LLM (scripted [`model::MockModel`] in
//!   tests; `AnthropicHttpModel` behind the `anthropic` feature; a rig-core
//!   adapter slots in the same way),
//! - [`tools::ToolExecutor`] — tool execution (path-jailed workspace fs
//!   tools included; bash policy-controlled),
//! - [`outcome::Grader`] — the outcome rubric grader.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod model;
pub mod outcome;
pub mod runtime;
pub mod tools;
pub mod turn;

pub use model::{MockModel, ModelClient, ModelRequest, ModelResponse};
pub use tools::{ToolExecutor, WorkspaceTools};
pub use turn::TurnRunner;
