//! The egress proxy's view of agentd: which session a proxy password is,
//! where that session may go, its credentials (values read from its vaults,
//! the sandbox only ever seeing a stable per-session placeholder), and the
//! CA its sandbox trusts. Engine-facing, the same rules give the sandbox its
//! environment.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use egress_proxy::{CaPem, Credential, EgressPolicy, SessionCa, SessionEgress};
use environments::EnvironmentsService;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sessions::SessionsService;
use shared::hash::Signer;
use shared::{ApiError, AuthContext, OrgId, WorkspaceId};
use vaults::egress::VaultSecrets;

/// Every proxy password starts with this.
const PASSWORD: &str = "agentd-px-";
/// How long a session's rules are reused before its vaults are read again
/// (a rotated secret takes effect within this; placeholders do not change).
const FRESH: Duration = Duration::from_secs(30);
/// What `allow_package_managers` opens under `limited` networking.
pub const PACKAGE_HOSTS: [&str; 9] = [
    "pypi.org",
    "files.pythonhosted.org",
    "registry.npmjs.org",
    "deb.debian.org",
    "security.debian.org",
    "dl-cdn.alpinelinux.org",
    "crates.io",
    "index.crates.io",
    "static.crates.io",
];

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    session: String,
    ws: String,
    org: String,
}

/// A credential as the agent may know it: name, purpose and hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disclosed {
    /// The variable name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// Where it may be sent (`None`: anywhere).
    pub hosts: Option<Vec<String>>,
}

/// Egress rules from sessions, environments and vaults.
pub struct SessionEgressPolicy {
    signer: Signer,
    sessions: Arc<SessionsService>,
    environments: Arc<EnvironmentsService>,
    vaults: VaultSecrets,
    internal_hosts: Vec<String>,
    cas: Mutex<HashMap<String, (CaPem, Arc<SessionCa>)>>,
    cache: Mutex<HashMap<String, (Instant, Arc<SessionEgress>)>>,
}

impl std::fmt::Debug for SessionEgressPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEgressPolicy").finish_non_exhaustive()
    }
}

impl SessionEgressPolicy {
    /// Rules signed under `signer`, over the services they read.
    pub fn new(
        signer: Signer,
        sessions: Arc<SessionsService>,
        environments: Arc<EnvironmentsService>,
        vaults: VaultSecrets,
        internal_hosts: Vec<String>,
    ) -> Self {
        Self {
            signer: signer.derive("egress"),
            sessions,
            environments,
            vaults,
            internal_hosts,
            cas: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// The proxy password of `session`.
    pub fn password(&self, ctx: &AuthContext, session: &str) -> String {
        let claims = Claims {
            session: session.to_owned(),
            ws: ctx.workspace.as_str().to_owned(),
            org: ctx.org.as_str().to_owned(),
        };
        self.signer.sign(PASSWORD, &claims)
    }

    /// The stand-in the sandbox sees for credential `id` of `session`.
    pub fn placeholder(&self, session: &str, id: &str) -> String {
        let mac = self.signer.mac_hex(&format!("{session}/{id}"));
        format!("agentd-ph-{}", mac.get(..32).unwrap_or(&mac))
    }

    /// The CA `session`'s sandbox trusts, made on first use.
    pub fn ca(&self, session: &str) -> Result<(CaPem, Arc<SessionCa>), ApiError> {
        if let Some(found) = self.cas.lock().get(session) {
            return Ok(found.clone());
        }
        let pem = CaPem::generate(session).map_err(|e| ApiError::internal(e.to_string()))?;
        let ca = Arc::new(SessionCa::from_pem(&pem).map_err(|e| ApiError::internal(e.to_string()))?);
        self.cas.lock().insert(session.to_owned(), (pem.clone(), ca.clone()));
        Ok((pem, ca))
    }

    /// The session's rules, read fresh.
    pub async fn rules(&self, ctx: &AuthContext, session: &str) -> Result<SessionEgress, ApiError> {
        let s = self.sessions.get(ctx, session).await?;
        let env = self.environments.get(ctx, &s.environment_id).await?;
        let config = serde_json::to_value(&env.config).unwrap_or_default();
        let allowed_hosts = (config.pointer("/networking/type").and_then(serde_json::Value::as_str)
            == Some("limited"))
        .then(|| {
            let mut hosts: Vec<String> = config
                .pointer("/networking/allowed_hosts")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|h| h.as_str().map(str::to_owned))
                .collect();
            if config.pointer("/networking/allow_package_managers").and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                hosts.extend(PACKAGE_HOSTS.iter().map(|h| (*h).to_owned()));
            }
            hosts
        });
        let secrets = self.vaults.environment_secrets(&ctx.workspace, &s.vault_ids).await?;
        let credentials = secrets
            .into_iter()
            .map(|e| Credential {
                placeholder: self.placeholder(session, &e.credential_id),
                id: e.credential_id,
                name: e.name,
                value: e.value.expose().to_owned(),
                hosts: e.hosts,
                header: e.header,
                body: e.body,
            })
            .collect();
        let (_, ca) = self.ca(session)?;
        Ok(SessionEgress {
            session: session.to_owned(),
            allowed_hosts,
            credentials,
            ca,
            internal_hosts: self.internal_hosts.clone(),
        })
    }

    /// What the sandbox of `session` is told: each credential's variable set
    /// to its placeholder, and the credentials as the agent may know them.
    pub async fn disclose(
        &self,
        ctx: &AuthContext,
        session: &str,
    ) -> Result<(BTreeMap<String, String>, Vec<Disclosed>), ApiError> {
        let s = self.sessions.get(ctx, session).await?;
        let secrets = self.vaults.environment_secrets(&ctx.workspace, &s.vault_ids).await?;
        let env =
            secrets.iter().map(|e| (e.name.clone(), self.placeholder(session, &e.credential_id))).collect();
        let disclosed = secrets
            .into_iter()
            .map(|e| Disclosed { name: e.name, description: e.description, hosts: e.hosts })
            .collect();
        Ok((env, disclosed))
    }
}

#[async_trait]
impl EgressPolicy for SessionEgressPolicy {
    async fn session(&self, password: &str) -> Option<Arc<SessionEgress>> {
        let claims: Claims = self.signer.verify(PASSWORD, password)?;
        if let Some((at, rules)) = self.cache.lock().get(&claims.session) {
            if at.elapsed() < FRESH {
                return Some(rules.clone());
            }
        }
        let ctx = AuthContext {
            org: OrgId::new(claims.org),
            workspace: WorkspaceId::new(claims.ws),
            api_key_id: "agentd-egress".into(),
        };
        let rules = Arc::new(self.rules(&ctx, &claims.session).await.ok()?);
        self.cache.lock().insert(claims.session, (Instant::now(), rules.clone()));
        Some(rules)
    }
}
