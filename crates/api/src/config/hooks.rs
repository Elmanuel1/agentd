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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_webhook_secret_under_24_bytes_stops_the_boot_and_names_the_endpoint() {
        // SH-49.
        let short = r#"[{"id": "ep_1", "organization_id": "o", "url": "https://h.test/in", "secret": "whsec_c2VjcmV0"}]"#;
        let refused =
            from_lookup(&|k| (k == "AGENTD_WEBHOOK_ENDPOINTS").then(|| short.to_owned())).unwrap_err();
        assert_eq!(refused.var, "AGENTD_WEBHOOK_ENDPOINTS");
        assert_eq!(refused.reason, "endpoint ep_1: the secret decodes to fewer than 24 bytes");
    }
}
