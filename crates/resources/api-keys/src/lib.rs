//! api-keys: the workspace API keys every non-admin request authenticates
//! with.
//!
//! A key acts for one organization and one workspace. Only its peppered
//! HMAC-SHA256 digest is stored ([`shared::ports::keys::material`]); the key
//! itself is returned once by [`ApiKeysService::create`]. The service is the
//! [`shared::ports::keys::KeySource`] the auth edge resolves keys through.
//! A self-hosted environment's keys ([`EnvironmentKeysService`]) work the same
//! way, and each reaches its own environment's work and sessions only.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod environment;
pub mod service;
pub mod store;
pub mod types;

pub use environment::EnvironmentKeysService;
pub use service::ApiKeysService;
pub use store::{ApiKeysStore, EnvironmentKeysStore, StoreError};
pub use types::{ApiKey, ApiKeyResource, EnvironmentKey, EnvironmentKeyResource};
