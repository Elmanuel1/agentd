//! `AGENTD_WEBHOOK_ENDPOINTS`: each organization's webhook endpoints, as a
//! JSON array of `{id, organization_id, url, secret, enabled}` (D20). Unset,
//! no webhook is sent.

use std::sync::Arc;

use webhooks::Webhooks;

use super::ConfigError;

pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Arc<Webhooks>, ConfigError> {
    match get("AGENTD_WEBHOOK_ENDPOINTS") {
        None => Ok(Arc::default()),
        Some(json) => Webhooks::parse(&json)
            .map(Arc::new)
            .map_err(|e| ConfigError { var: "AGENTD_WEBHOOK_ENDPOINTS", reason: e.to_string() }),
    }
}
