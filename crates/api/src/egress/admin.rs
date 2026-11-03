//! The egress service's admin interface, as agentd calls it: register a
//! session, and drop it when its sandbox is gone.

use egress_proxy::{Registered, Registration};
use shared::ApiError;

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

    /// Register `session` (again); the CA its sandbox trusts.
    pub async fn register(&self, session: &str, registration: &Registration) -> Result<Registered, ApiError> {
        let sent = self
            .http
            .put(self.session_url(session))
            .bearer_auth(&self.token)
            .json(registration)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| ApiError::internal(format!("egress service: {e}")))?;
        sent.json().await.map_err(|e| ApiError::internal(format!("egress service: {e}")))
    }

    /// Drop `session`, in the background: its sandbox is gone either way.
    pub fn forget(&self, session: &str) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        let req = self.http.delete(self.session_url(session)).bearer_auth(&self.token);
        let session = session.to_owned();
        rt.spawn(async move {
            if let Err(e) = req.send().await.and_then(reqwest::Response::error_for_status) {
                tracing::warn!(%session, error = %e, "the egress service did not drop the session");
            }
        });
    }
}
