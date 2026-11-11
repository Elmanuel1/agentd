//! engine: the agent loop. A loop worker claims a session's step from the
//! queue, builds one model call's prompt from the session's event log, calls
//! the model, and commits what the reply leads to. Built-in tool calls are
//! parked for the session's worker; nothing is held between steps.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod json;
pub mod model;
pub mod turn;
pub mod worker;
