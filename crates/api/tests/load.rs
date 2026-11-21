//! Load tests, run by hand: `cargo test -p api --test load -- --ignored --nocapture`.
//! Gated by `AGENTD_PG_TESTS=1`; the feed's wakes need a running Debezium Server.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;
#[path = "load/webhooks.rs"]
mod webhooks;
