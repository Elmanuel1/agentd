//! What the proxy asks agentd about a session: where it may go, which
//! credentials it holds, and the CA its sandbox trusts.

use std::sync::Arc;

use async_trait::async_trait;

use crate::ca::SessionCa;
use crate::rules::Credential;

/// One session's egress rules.
#[derive(Debug)]
pub struct SessionEgress {
    /// The session.
    pub session: String,
    /// Hosts a `limited` environment allows (`None`: unrestricted).
    pub allowed_hosts: Option<Vec<String>>,
    /// The credentials it may use.
    pub credentials: Vec<Credential>,
    /// The CA its sandbox trusts.
    pub ca: Arc<SessionCa>,
    /// Hosts the operator allows at internal addresses
    /// (`AGENTD_EGRESS_INTERNAL_HOSTS`); every other host must resolve to
    /// public addresses only.
    pub internal_hosts: Vec<String>,
}

impl SessionEgress {
    /// Whether the session may reach `host` at all: under `limited`
    /// networking only the environment's hosts (a credential's hosts must be
    /// allowed there too, as documented).
    pub fn may_reach(&self, host: &str) -> bool {
        self.allowed_hosts.as_ref().is_none_or(|hosts| crate::rules::any_matches(hosts, host))
    }
}

/// agentd's side of the proxy.
#[async_trait]
pub trait EgressPolicy: Send + Sync {
    /// The session a proxy password belongs to, or `None` for a password
    /// agentd did not issue.
    async fn session(&self, password: &str) -> Option<Arc<SessionEgress>>;
}
