//! egress-proxy: sandbox traffic leaves through agentd. The proxy knows the
//! session from its proxy password, blocks hosts the session may not reach,
//! and, for a host a credential allows, swaps the credential's placeholder
//! in the request headers for the real value; a placeholder bound anywhere
//! else is refused, and a real value echoed in a response is masked again.
//! The sandbox only ever sees placeholders, and trusts a CA of its own
//! session, which lets the proxy read HTTPS.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing))]

pub mod ca;
mod forward;
pub mod net;
pub mod policy;
pub mod rules;
mod server;

pub use ca::{CaPem, SessionCa};
pub use policy::{EgressPolicy, SessionEgress};
pub use rules::Credential;
pub use server::serve;
