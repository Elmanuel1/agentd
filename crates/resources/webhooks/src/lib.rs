//! webhooks: the deliveries live sends an organization's endpoints, built
//! and signed as live builds them (Standard Webhooks, `user-agent:
//! Claude-Platform`). Each delivery row is written in the commit of the
//! change it reports; the change feed hands each new row to [`WebhookSender`],
//! which posts it, retries twice about 31 s apart, then drops it (H-1 to H-4).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

mod endpoints;
mod event;
mod sender;
mod sign;

pub use endpoints::{Endpoint, EndpointsError, Webhooks};
pub use event::{Delivery, EventKind};
pub use sender::{DeliveryMarks, NativeDeliveryMarks, WebhookSender};
pub use sign::{SigningSecret, sign};
