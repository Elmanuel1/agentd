//! egress-proxy: sandbox traffic leaves through this proxy. The proxy user
//! names the session; the proxy blocks hosts the session may not reach and,
//! for a host a credential allows, swaps the credential's placeholder in the
//! request headers for its value, read from the credential store for that
//! one request and zeroed once it is sent. A placeholder bound anywhere else
//! is refused, and a value echoed in a response is masked. The sandbox only
//! ever sees placeholders, and trusts a CA of its own session, which lets the
//! proxy read HTTPS. agentd registers sessions through the admin interface.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod admin;
pub mod ca;
mod forward;
pub mod inject;
pub mod locked;
pub mod mask;
pub mod net;
pub mod policy;
pub mod registry;
mod response;
pub mod rules;
pub mod secret;
mod server;

pub use ca::{CaPem, SessionCa};
pub use locked::Locked;
pub use policy::{EgressPolicy, SessionEgress};
pub use registry::{BindingSpec, Registered, Registration, Registry};
pub use rules::Binding;
pub use secret::{Current, SecretSource};
pub use server::serve;
