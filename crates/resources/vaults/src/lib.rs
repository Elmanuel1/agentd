//! vaults — Vault + Credential entities.
//!
//! Security invariant, enforced by the type system: secret material lives in
//! [`types::SecretString`], which deliberately has **no `Serialize` impl** and
//! a redacting `Debug`. Wire responses are built from projection types that
//! cannot contain secrets — leaking one is a compile error.
//!
//! Live-verified: vault wire shape (`vlt_…`, `display_name`, no scope field —
//! workspace-scoped implicitly). Credential response shape follows docs
//! (write-only secrets, unique key per vault → 409, ≤20 per vault,
//! immutable keys, archive purges the secret).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod egress;
pub mod routes;
pub mod sealed;
pub mod service;
pub mod store;
pub mod types;

use std::sync::Arc;

use shared::beta::BetaGate;
use shared::Module;

pub use service::VaultsService;
pub use store::VaultsStore;
pub use types::{Credential, SecretString, Vault, VaultId};

/// The vaults module: its routes, behind the Managed Agents beta.
pub fn module(service: Arc<VaultsService>) -> Module {
    Module::new(routes::router(), service).with_beta(BetaGate::managed(&["/v1/vaults"]))
}
