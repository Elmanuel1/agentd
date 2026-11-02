//! engine — the ORCHESTRATOR-side agent loop.
//!
//! Ownership split (matching the Managed Agents architecture: the agent
//! loop never runs in the sandbox — the container only executes tools):
//!
//! - **Orchestrator (this crate's [`turn::TurnRunner`] / [`outcome::OutcomeLoop`])**:
//!   drains user events, calls the model (holds the API key), evaluates the
//!   permission policy, parks/resumes on ask/custom, journals every wire
//!   event through [`sessions::EngineContract`], drives grading.
//!   The policy gate MUST live here — a sandbox cannot be trusted to
//!   enforce its own `always_ask`.
//! - **Sandbox (the ephemeral Fargate/Firecracker/local task)**: implements
//!   [`tools::ToolExecutor`] and nothing else — receives `(name, input)`,
//!   returns output from inside the path jail. A dumb executor, like the
//!   self-hosted `EnvironmentWorker`.
//!
//! `ToolExecutor` is therefore the orchestrator↔sandbox seam:
//! [`tools::WorkspaceTools`] is the in-process impl (dev/local); a
//! `RemoteToolExecutor` dispatching over the task work channel is the
//! production impl (with the same jailed `WorkspaceTools` answering inside
//! the task).
//!
//! Every non-deterministic dependency is a port:
//! - [`model::ModelClient`] — the LLM (scripted [`model::MockModel`] in
//!   tests; `AnthropicHttpModel` behind the `anthropic` feature; a rig-core
//!   adapter slots in the same way),
//! - [`tools::ToolExecutor`] — tool execution (the sandbox seam),
//! - [`outcome::Grader`] — the outcome rubric grader (independent context,
//!   orchestrator-side by definition).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod model;
pub mod outcome;
pub mod runtime;
pub mod tools;
pub mod turn;

pub use model::{MockModel, ModelClient, ModelRequest, ModelResponse};
pub use tools::{ToolExecutor, WorkspaceTools};
pub use turn::TurnRunner;
