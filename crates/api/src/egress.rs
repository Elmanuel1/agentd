//! agentd's side of the egress service: each session's credentials as the
//! sandbox and the agent may know them (names, placeholders and hosts, never
//! a value), registered with the service, which answers with the CA the
//! sandbox trusts. agentd reads no credential value for egress.

use std::collections::BTreeMap;
use std::sync::Arc;

use egress_proxy::{BindingSpec, Registered, Registration};
use environments::EnvironmentsService;
use serde::Serialize;
use sessions::SessionsService;
use sha2::{Digest, Sha256};
use shared::hash::Signer;
use shared::{ApiError, AuthContext};
use vaults::egress::VaultCredentials;

/// Every proxy password starts with this.
const PASSWORD: &str = "agentd-px-";
#[derive(Serialize)]
struct Claims<'a> {
    session: &'a str,
    ws: &'a str,
}

/// A credential as the agent may know it: name, purpose and hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disclosed {
    /// The variable name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// Where it may be sent (`None`: nowhere).
    pub hosts: Option<Vec<String>>,
}

/// A session's egress, as its sandbox is set up with it.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The proxy password.
    pub password: String,
    /// Each credential's variable, set to its placeholder.
    pub env: BTreeMap<String, String>,
    /// The credentials as the agent may know them.
    pub disclosed: Vec<Disclosed>,
    /// The session CA's certificate, PEM.
    pub ca_cert: String,
}

/// The egress service's admin interface.
#[derive(Clone)]
pub struct EgressAdmin {
    http: reqwest::Client,
    url: String,
    token: String,
}

impl std::fmt::Debug for EgressAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EgressAdmin").field("url", &self.url).finish_non_exhaustive()
    }
}

impl EgressAdmin {
    /// The admin interface at `url`, called with `token`.
    pub fn new(url: &str, token: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { http, url: url.trim_end_matches('/').to_owned(), token: token.to_owned() }
    }

    fn session_url(&self, session: &str) -> String {
        format!("{}/v1/sessions/{session}", self.url)
    }
}

/// Registers sessions with the egress service.
pub struct SessionEgressPolicy {
    signer: Signer,
    sessions: Arc<SessionsService>,
    environments: Arc<EnvironmentsService>,
    vaults: VaultCredentials,
    admin: EgressAdmin,
}

impl std::fmt::Debug for SessionEgressPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEgressPolicy").field("admin", &self.admin).finish_non_exhaustive()
    }
}

impl SessionEgressPolicy {
    /// Passwords and placeholders signed under `signer`, from the services
    /// they read, registered through `admin`.
    pub fn new(
        signer: Signer,
        sessions: Arc<SessionsService>,
        environments: Arc<EnvironmentsService>,
        vaults: VaultCredentials,
        admin: EgressAdmin,
    ) -> Self {
        Self { signer: signer.derive("egress"), sessions, environments, vaults, admin }
    }

    /// The proxy password of `session`: stable, so a registration again
    /// leaves the sandbox's settings valid.
    pub fn password(&self, ctx: &AuthContext, session: &str) -> String {
        self.signer.sign(PASSWORD, &Claims { session, ws: ctx.workspace.as_str() })
    }

    /// The stand-in the sandbox sees for credential `id` of `session`.
    pub fn placeholder(&self, session: &str, id: &str) -> String {
        let mac = self.signer.mac_hex(&format!("{session}/{id}"));
        format!("agentd-ph-{}", mac.get(..32).unwrap_or(&mac))
    }

    /// Register `session` with the egress service as it stands now: its
    /// environment's hosts and its vaults' credentials.
    pub async fn prepare(&self, ctx: &AuthContext, session: &str) -> Result<Prepared, ApiError> {
        let s = self.sessions.get(ctx, session).await?;
        let env = self.environments.get(ctx, &s.environment_id).await?;
        let config = serde_json::to_value(&env.config).unwrap_or_default();
        let credentials = self.vaults.environment_credentials(&ctx.workspace, &s.vault_ids).await?;
        let password = self.password(ctx, session);
        let registration = Registration {
            password_sha256: hex::encode(Sha256::digest(password.as_bytes())),
            allowed_hosts: environments::networking::allowed_hosts(&config),
            credentials: credentials
                .iter()
                .map(|c| BindingSpec {
                    id: c.credential_id.clone(),
                    name: c.name.clone(),
                    placeholder: self.placeholder(session, &c.credential_id),
                    workspace: ctx.workspace.as_str().to_owned(),
                    vault: c.vault_id.clone(),
                })
                .collect(),
        };
        let sent = self
            .admin
            .http
            .put(self.admin.session_url(session))
            .bearer_auth(&self.admin.token)
            .json(&registration)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status);
        let registered: Registered = match sent {
            Ok(r) => r.json().await.map_err(|e| ApiError::internal(format!("egress service: {e}")))?,
            Err(e) => return Err(ApiError::internal(format!("egress service: {e}"))),
        };
        Ok(Prepared {
            password,
            env: credentials
                .iter()
                .map(|c| (c.name.clone(), self.placeholder(session, &c.credential_id)))
                .collect(),
            disclosed: credentials
                .into_iter()
                .map(|c| Disclosed { name: c.name, description: c.description, hosts: c.hosts })
                .collect(),
            ca_cert: registered.ca_cert,
        })
    }

    /// Tell the egress service `session`'s sandbox is gone.
    pub fn forget(&self, session: &str) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        let req = self.admin.http.delete(self.admin.session_url(session)).bearer_auth(&self.admin.token);
        let session = session.to_owned();
        rt.spawn(async move {
            if let Err(e) = req.send().await.and_then(reqwest::Response::error_for_status) {
                tracing::warn!(%session, error = %e, "the egress service did not drop the session");
            }
        });
    }
}
