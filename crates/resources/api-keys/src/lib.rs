//! api-keys: the workspace API keys every non-admin request authenticates
//! with.
//!
//! A key acts for one organization and one workspace. Only its peppered
//! HMAC-SHA256 digest is stored ([`shared::ports::keys::material`]); the key
//! itself is returned once by [`ApiKeysService::create`]. The service is the
//! [`shared::ports::keys::KeySource`] the auth edge resolves keys through.
//! A self-hosted environment's keys ([`EnvironmentKeysService`]) and an
//! organization's admin keys ([`AdminKeysService`]) work the same way. Every
//! digest is keyed with the current pepper of a [`shared::ports::keys::PepperRing`],
//! and a key stored under an older pepper is found under it and moved.
//! [`CachedKeys`] keeps resolved workspace keys for 60 s, dropping a key as
//! soon as [`KeyRevokes`] hears of its revoke on the change feed.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod admin;
mod cache;
pub mod environment;
pub mod service;
pub mod store;
pub mod types;

pub use admin::AdminKeysService;
pub use cache::{CachedKeys, KeyRevokes, REVOKED, Report};
pub use environment::EnvironmentKeysService;
pub use service::ApiKeysService;
pub use store::{AdminKeysStore, ApiKeysStore, EnvironmentKeysStore, KeyDigests, StoreError};
pub use types::{
    AdminKey, AdminKeyResource, ApiKey, ApiKeyResource, EnvironmentKey, EnvironmentKeyResource, Keyed,
};
