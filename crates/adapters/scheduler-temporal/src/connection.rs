//! Where and how agentd reaches Temporal: TLS when the address is
//! `https://`, its certificate chaining to the CA file when one is given, else
//! to the operating system's roots, and an API key (Temporal Cloud's) sent as
//! a bearer token on every call.

use shared::ports::SchedulerError;
use temporalio_client::{ConnectionOptions, TlsOptions, Url};

/// Where agentd reaches Temporal.
#[derive(Clone, PartialEq, Eq)]
pub struct TemporalConfig {
    /// The frontend, `http://host:7233` or `https://host:7233`.
    pub address: String,
    /// The namespace the schedules live in.
    pub namespace: String,
    /// The task queue agentd's worker polls.
    pub task_queue: String,
    /// The API key every call carries.
    pub api_key: Option<String>,
    /// The PEM file an `https://` frontend's certificate chains to.
    pub server_ca: Option<String>,
}

/// Shows whether an API key is set, never the key.
impl std::fmt::Debug for TemporalConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemporalConfig")
            .field("address", &self.address)
            .field("namespace", &self.namespace)
            .field("task_queue", &self.task_queue)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("server_ca", &self.server_ca)
            .finish()
    }
}

impl TemporalConfig {
    pub(crate) fn connection(&self) -> Result<ConnectionOptions, SchedulerError> {
        let url = Url::parse(&self.address).map_err(crate::unavailable)?;
        let tls = match url.scheme() {
            "https" => {
                let ca =
                    self.server_ca.as_ref().map(std::fs::read).transpose().map_err(crate::unavailable)?;
                Some(TlsOptions::builder().maybe_server_root_ca_cert(ca).build())
            }
            _ => None,
        };
        Ok(ConnectionOptions::new(url).maybe_tls_options(tls).maybe_api_key(self.api_key.clone()).build())
    }
}

#[cfg(test)]
mod tests;
