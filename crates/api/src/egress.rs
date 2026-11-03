//! agentd's side of the egress service: each session's credentials as the
//! sandbox and the agent may know them (names, placeholders and hosts, never
//! a value), registered with the service, which answers with the CA the
//! sandbox trusts. agentd reads no credential value for egress.

use std::collections::BTreeMap;

use egress_proxy::{BindingSpec, Registration};
use serde::Serialize;
use sha2::{Digest, Sha256};
use shared::hash::Signer;
use shared::{ApiError, AuthContext};
use vaults::egress::VaultCredentials;

use crate::model_key;

mod admin;

pub use admin::EgressAdmin;

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

/// Registers sessions with the egress service.
pub struct SessionEgressPolicy {
    signer: Signer,
    vaults: VaultCredentials,
    admin: EgressAdmin,
    /// The host the harness calls the model at, reachable under any networking.
    model_host: Option<String>,
}

impl std::fmt::Debug for SessionEgressPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEgressPolicy").field("admin", &self.admin).finish_non_exhaustive()
    }
}

impl SessionEgressPolicy {
    /// Passwords and placeholders signed under `signer`, credentials listed
    /// from `vaults`, registered through `admin`; the harness calls the model
    /// at `model_base_url`.
    pub fn new(signer: Signer, vaults: VaultCredentials, admin: EgressAdmin, model_base_url: &str) -> Self {
        let model_host =
            reqwest::Url::parse(model_base_url).ok().and_then(|u| u.host_str().map(str::to_owned));
        Self { signer: signer.derive("egress"), vaults, admin, model_host }
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

    /// Register `session` with the egress service: its environment's hosts
    /// (`environment` is its config) and the credentials of `vault_ids`.
    pub async fn prepare(
        &self,
        ctx: &AuthContext,
        session: &str,
        vault_ids: &[String],
        environment: &serde_json::Value,
    ) -> Result<Prepared, ApiError> {
        let credentials = self.vaults.environment_credentials(&ctx.workspace, vault_ids).await?;
        let password = self.password(ctx, session);
        let registration = Registration {
            password_sha256: hex::encode(Sha256::digest(password.as_bytes())),
            // The harness's own model calls leave whatever the agent may reach.
            allowed_hosts: environments::networking::allowed_hosts(environment)
                .map(|hosts| hosts.into_iter().chain(self.model_host.clone()).collect()),
            credentials: credentials
                .iter()
                .map(|c| BindingSpec {
                    id: c.credential_id.clone(),
                    name: c.name.clone(),
                    placeholder: self.placeholder(session, &c.credential_id),
                    workspace: ctx.workspace.as_str().to_owned(),
                    vault: c.vault_id.clone(),
                })
                .chain([self.model_key(session)])
                .collect(),
        };
        let registered = self.admin.register(session, &registration).await?;
        Ok(Prepared {
            password,
            env: credentials
                .iter()
                .map(|c| (c.name.clone(), self.placeholder(session, &c.credential_id)))
                .chain([(model_key::VARIABLE.to_owned(), self.placeholder(session, model_key::KEY.2))])
                .collect(),
            disclosed: credentials
                .into_iter()
                .map(|c| Disclosed { name: c.name, description: c.description, hosts: c.hosts })
                .collect(),
            ca_cert: registered.ca_cert,
        })
    }

    /// The operator's model key, which every session's harness calls the
    /// model with; the agent's prompt does not list it.
    fn model_key(&self, session: &str) -> BindingSpec {
        let (workspace, vault, id) = model_key::KEY;
        BindingSpec {
            id: id.to_owned(),
            name: model_key::VARIABLE.to_owned(),
            placeholder: self.placeholder(session, id),
            workspace: workspace.to_owned(),
            vault: vault.to_owned(),
        }
    }

    /// Tell the egress service `session`'s sandbox is gone.
    pub fn forget(&self, session: &str) {
        self.admin.forget(session);
    }
}
