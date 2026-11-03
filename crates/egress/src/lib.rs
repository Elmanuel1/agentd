//! agentd-egress: the egress proxy as a service of its own. Sandboxes reach
//! its proxy port; agentd registers sessions on its admin port. It reads a
//! credential's value from Secrets Manager each time a request needs it,
//! with a role that may do nothing else.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod config;
pub mod harden;
pub mod source;
