//! What each session's sandbox starts with: the work item its harness runs,
//! where the harness reaches agentd, and its egress settings, registered with
//! the egress service at launch.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sessions::types::{Session, SessionId};
use sessions::SandboxWork;
use shared::{ApiError, AuthContext, WorkspaceId};
use work::WorkService;

use crate::egress::SessionEgressPolicy;

/// The session CA, as the sandbox's trust bundles name it.
const CA_PATH: &str = "/etc/agentd/egress-ca.pem";
/// The system CAs and the session CA together.
const CA_BUNDLE: &str = "/tmp/agentd/ca-bundle.pem";

/// [`SandboxWork`] over the work service and the egress service.
#[derive(Debug, Clone)]
pub struct WorkIssuer {
    /// The work items.
    pub work: Arc<WorkService>,
    /// agentd's API as the sandbox reaches it.
    pub api_url: String,
    /// Registers sessions with the egress service; `None` when there is none.
    pub egress: Option<Arc<SessionEgressPolicy>>,
    /// The egress proxy as the sandbox reaches it (`host:port`).
    pub egress_host: String,
    /// The model API the harness calls (`AGENTD_MODEL_BASE_URL`).
    pub model_base_url: String,
}

impl WorkIssuer {
    /// The egress part of `session`'s environment: each credential's
    /// placeholder, the proxy, the CA and the credentials as the agent may
    /// know them. Empty with no egress service: the sandbox reaches nothing
    /// beyond agentd.
    async fn egress_env(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<BTreeMap<String, String>, ApiError> {
        let Some(policy) = &self.egress else { return Ok(BTreeMap::new()) };
        let id = session.id.as_str();
        let prepared = policy.prepare(ctx, id, &session.vault_ids, environment).await?;
        let mut env = prepared.env;
        let proxy = format!("http://{id}:{}@{}", prepared.password, self.egress_host);
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy", "ALL_PROXY"] {
            env.insert(name.to_owned(), proxy.clone());
        }
        // agentd itself is reached directly, never through the proxy.
        let api_host = reqwest::Url::parse(&self.api_url).ok().and_then(|u| u.host_str().map(str::to_owned));
        let direct = ["localhost", "127.0.0.1"].into_iter().map(str::to_owned).chain(api_host);
        let direct = direct.collect::<Vec<_>>().join(",");
        for name in ["NO_PROXY", "no_proxy"] {
            env.insert(name.to_owned(), direct.clone());
        }
        for name in ["SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"] {
            env.insert(name.to_owned(), CA_BUNDLE.to_owned());
        }
        env.insert("NODE_EXTRA_CA_CERTS".into(), CA_PATH.to_owned());
        env.insert("AGENTD_EGRESS_CA".into(), prepared.ca_cert);
        let disclosed: Vec<_> = prepared
            .disclosed
            .into_iter()
            .map(|d| serde_json::json!({"name": d.name, "description": d.description, "hosts": d.hosts}))
            .collect();
        env.insert("AGENTD_CREDENTIALS".into(), serde_json::Value::from(disclosed).to_string());
        Ok(env)
    }
}

#[async_trait]
impl SandboxWork for WorkIssuer {
    async fn issue(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<BTreeMap<String, String>, ApiError> {
        let mut env = self.egress_env(ctx, session, environment).await?;
        let secret = self.work.issue(ctx, &session.environment_id, session.id.as_str()).await?;
        env.extend([
            ("AGENTD_API_URL".to_owned(), self.api_url.clone()),
            ("AGENTD_ENVIRONMENT_ID".to_owned(), session.environment_id.clone()),
            ("AGENTD_WORK_ID".to_owned(), session.id.as_str().to_owned()),
            ("AGENTD_WORK_SECRET".to_owned(), secret),
            ("AGENTD_MODEL_BASE_URL".to_owned(), self.model_base_url.clone()),
        ]);
        Ok(env)
    }

    async fn queue(
        &self,
        ctx: &AuthContext,
        session: &Session,
        environment: &serde_json::Value,
    ) -> Result<(), ApiError> {
        let self_hosted = environment.get("type").and_then(serde_json::Value::as_str) == Some("self_hosted");
        self.work.queue_for_loop(ctx, &session.environment_id, session.id.as_str(), self_hosted).await
    }

    async fn end(&self, ws: &WorkspaceId, session: &SessionId) {
        if let Err(e) = self.work.end_session(ws, session.as_str()).await {
            tracing::warn!(session = %session, error = %e.message, "work: could not stop the session's work item");
        }
        self.forget(session);
    }

    fn forget(&self, session: &SessionId) {
        if let Some(egress) = &self.egress {
            egress.forget(session.as_str());
        }
    }
}
