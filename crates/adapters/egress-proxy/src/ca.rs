//! A session's own certificate authority: the sandbox trusts it, and the
//! proxy signs a certificate for each host the sandbox calls, so it can read
//! the request (to put secrets in) and the response (to take them out).

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair, KeyUsagePurpose};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;

/// A CA as PEM, the form it is stored in.
#[derive(Clone, PartialEq, Eq)]
pub struct CaPem {
    /// The certificate, which the sandbox trusts.
    pub cert: String,
    /// The private key, which never leaves the proxy.
    pub key: String,
}

impl std::fmt::Debug for CaPem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaPem").finish_non_exhaustive()
    }
}

/// Errors making or using a CA.
#[derive(Debug, thiserror::Error)]
pub enum CaError {
    /// Certificate generation or parsing failed.
    #[error("certificate: {0}")]
    Cert(#[from] rcgen::Error),
    /// The TLS configuration was refused.
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
}

impl CaPem {
    /// A new CA for `session`.
    pub fn generate(session: &str) -> Result<Self, CaError> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.distinguished_name.push(DnType::CommonName, format!("agentd egress {session}"));
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(365);
        let cert = params.self_signed(&key)?;
        Ok(Self { cert: cert.pem(), key: key.serialize_pem() })
    }
}

/// Most leaf certificates one session keeps; past it the cache starts over,
/// so a sandbox calling host after host cannot grow the shared service.
const MAX_LEAVES: usize = 256;

/// A CA ready to sign, with the server configs it made, by host.
pub struct SessionCa {
    issuer_params: CertificateParams,
    issuer_key: KeyPair,
    leaves: Mutex<HashMap<String, Arc<ServerConfig>>>,
}

impl std::fmt::Debug for SessionCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCa").finish_non_exhaustive()
    }
}

impl SessionCa {
    /// Load a stored CA.
    pub fn from_pem(pem: &CaPem) -> Result<Self, CaError> {
        let issuer_key = KeyPair::from_pem(&pem.key)?;
        let issuer_params = CertificateParams::from_ca_cert_pem(&pem.cert)?;
        Ok(Self { issuer_params, issuer_key, leaves: Mutex::new(HashMap::new()) })
    }

    /// A TLS server config presenting a certificate for `host`, signed by
    /// this CA.
    pub fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>, CaError> {
        if let Some(c) = self.leaves.lock().get(host) {
            return Ok(c.clone());
        }
        let issuer = self.issuer_params.clone().self_signed(&self.issuer_key)?;
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec![host.to_owned()])?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(30);
        let leaf = params.signed_by(&key, &issuer, &self.issuer_key)?;
        let chain = vec![CertificateDer::from(leaf.der().to_vec())];
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        // An explicit provider: the workspace builds rustls with more than one,
        // and then rustls picks no default.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(chain, private)?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let config = Arc::new(config);
        let mut leaves = self.leaves.lock();
        if leaves.len() >= MAX_LEAVES {
            leaves.clear();
        }
        leaves.insert(host.to_owned(), config.clone());
        Ok(config)
    }
}
