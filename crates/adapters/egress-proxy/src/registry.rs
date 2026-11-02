//! The sessions agentd has registered: each one's password digest, rules and
//! CA. The CA's key is made here and never leaves the proxy; a session
//! registered again keeps its CA, so its sandbox goes on trusting it.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shared::ports::secrets::SecretKey;

use crate::ca::{CaError, CaPem, SessionCa};
use crate::policy::{EgressPolicy, SessionEgress};
use crate::rules::Binding;

/// A credential as agentd registers it: where its value lives, never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingSpec {
    /// The vault credential's id.
    pub id: String,
    /// The variable name the sandbox sees.
    pub name: String,
    /// The stand-in the sandbox sees.
    pub placeholder: String,
    /// The credential's workspace.
    pub workspace: String,
    /// The credential's vault.
    pub vault: String,
}

/// What agentd registers for a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    /// SHA-256 of the session's proxy password, hex.
    pub password_sha256: String,
    /// Hosts a `limited` environment allows (`None`: unrestricted).
    pub allowed_hosts: Option<Vec<String>>,
    /// The credentials the session holds.
    pub credentials: Vec<BindingSpec>,
}

/// What the proxy answers a registration with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// The session CA's certificate, PEM, for the sandbox to trust.
    pub ca_cert: String,
}

/// A registration the proxy refuses.
#[derive(Debug, thiserror::Error)]
pub enum RegisterError {
    /// A credential names a workspace, vault or credential outside the id charset.
    #[error("credential {0}: invalid key")]
    Key(String),
    /// The password digest is not 32 bytes of hex.
    #[error("password_sha256 must be 64 hex digits")]
    Digest,
    /// The CA could not be made.
    #[error(transparent)]
    Ca(#[from] CaError),
}

struct Entry {
    password: [u8; 32],
    egress: Arc<SessionEgress>,
    ca_cert: String,
}

/// The registered sessions.
pub struct Registry {
    internal_hosts: Vec<String>,
    sessions: Mutex<HashMap<String, Entry>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("sessions", &self.sessions.lock().len()).finish_non_exhaustive()
    }
}

impl Registry {
    /// An empty registry; `internal_hosts` may be reached at internal addresses.
    pub fn new(internal_hosts: Vec<String>) -> Self {
        Self { internal_hosts, sessions: Mutex::new(HashMap::new()) }
    }

    /// Register `session` (again); its CA certificate.
    pub fn register(&self, session: &str, reg: Registration) -> Result<Registered, RegisterError> {
        let mut password = [0u8; 32];
        hex::decode_to_slice(&reg.password_sha256, &mut password).map_err(|_| RegisterError::Digest)?;
        let bindings = reg
            .credentials
            .into_iter()
            .map(|c| {
                let key = SecretKey::new(&c.workspace, &c.vault, &c.id)
                    .map_err(|_| RegisterError::Key(c.id.clone()))?;
                Ok(Binding { id: c.id, name: c.name, placeholder: c.placeholder, key })
            })
            .collect::<Result<Vec<_>, RegisterError>>()?;
        let known = self.sessions.lock().get(session).map(|e| (e.egress.ca.clone(), e.ca_cert.clone()));
        let (ca, ca_cert) = match known {
            Some(found) => found,
            None => {
                let pem = CaPem::generate(session)?;
                (Arc::new(SessionCa::from_pem(&pem)?), pem.cert)
            }
        };
        let egress = Arc::new(SessionEgress {
            session: session.to_owned(),
            allowed_hosts: reg.allowed_hosts,
            bindings,
            ca,
            internal_hosts: self.internal_hosts.clone(),
        });
        let entry = Entry { password, egress, ca_cert: ca_cert.clone() };
        self.sessions.lock().insert(session.to_owned(), entry);
        Ok(Registered { ca_cert })
    }

    /// Drop `session`: its sandbox is gone.
    pub fn forget(&self, session: &str) {
        self.sessions.lock().remove(session);
    }
}

#[async_trait]
impl EgressPolicy for Registry {
    async fn session(&self, session: &str, password: &str) -> Option<Arc<SessionEgress>> {
        let digest: [u8; 32] = Sha256::digest(password.as_bytes()).into();
        let sessions = self.sessions.lock();
        let entry = sessions.get(session)?;
        (entry.password == digest).then(|| entry.egress.clone())
    }
}
