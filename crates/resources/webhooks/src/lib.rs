//! webhooks: the deliveries live sends an organization's endpoints, built
//! and signed as live builds them (Standard Webhooks, `user-agent:
//! Claude-Platform`). Each delivery row is written in the commit of the
//! change it reports; the change feed only wakes the instances, and every
//! instance's [`WebhookSender`] claims due rows and posts them, three
//! attempts with jittered backoff, then drops them (H-1, H-2, SH-1 to SH-4).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod endpoints;
mod event;
pub mod queue;
mod sender;
mod sign;
pub mod states;

pub use endpoints::{Endpoint, EndpointsError, Webhooks};
pub use event::{Delivery, EventKind};
pub use sender::{DeliveryWake, NoMetrics, SenderMetrics, SenderPorts, WAKE, WebhookSender};
pub use sign::{SigningSecret, sign};
