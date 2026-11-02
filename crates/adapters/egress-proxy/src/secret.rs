//! Where the proxy reads a credential's value: on every request that uses
//! it, never from a cache, so a value changed in the store applies to the
//! next request. A read that fails refuses the request.

use async_trait::async_trait;
use shared::ports::secrets::{SecretError, SecretKey};

use crate::locked::Locked;
use crate::rules::any_matches;

/// A credential as it stands now.
#[derive(Debug)]
pub struct Current {
    /// The value, locked in RAM and zeroed on drop.
    pub value: Locked,
    /// Hosts it may be sent to. One that names none is sent nowhere: "any
    /// host" would include one the agent controls.
    pub hosts: Option<Vec<String>>,
    /// Put into request headers (the only place the proxy puts a value).
    pub header: bool,
}

impl Current {
    /// Whether the value may be sent to `host`.
    pub fn allows(&self, host: &str) -> bool {
        self.hosts.as_ref().is_some_and(|h| any_matches(h, host))
    }
}

/// The credential store, read-only.
#[async_trait]
pub trait SecretSource: Send + Sync {
    /// The current state of the credential at `key`.
    async fn current(&self, key: &SecretKey) -> Result<Current, SecretError>;
}
