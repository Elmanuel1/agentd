//! The endpoints each organization's deliveries go to, from
//! `AGENTD_WEBHOOK_ENDPOINTS` (D20): live registers them in its Console, and
//! how a deployment fills the setting is its own concern.

use std::collections::HashMap;

use serde::Deserialize;

use super::event::{Delivery, EventKind, body};
use super::sign::SigningSecret;

/// One configured endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Its key, unique among the endpoints.
    pub key: String,
    /// The organization whose events it receives.
    pub organization_id: String,
    /// Where deliveries are posted.
    pub url: String,
    /// What signs them.
    pub secret: SigningSecret,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    id: String,
    organization_id: String,
    url: String,
    secret: String,
    #[serde(default = "enabled")]
    enabled: bool,
}

fn enabled() -> bool {
    true
}

/// Why `AGENTD_WEBHOOK_ENDPOINTS` did not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EndpointsError {
    /// Not a JSON array of endpoints.
    #[error("expected a JSON array of {{id, organization_id, url, secret, enabled}}: {0}")]
    Shape(String),
    /// A secret that is not `whsec_<base64>`.
    #[error("endpoint {0}: the secret is not whsec_<base64>")]
    Secret(String),
    /// Two endpoints with one id.
    #[error("endpoint {0} is listed twice")]
    Duplicate(String),
}

/// The enabled endpoints, by organization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Webhooks {
    by_org: HashMap<String, Vec<Endpoint>>,
    by_key: HashMap<String, Endpoint>,
}

impl Webhooks {
    /// The endpoints `json` lists; disabled ones are left out.
    pub fn parse(json: &str) -> Result<Self, EndpointsError> {
        let raw: Vec<Raw> = serde_json::from_str(json).map_err(|e| EndpointsError::Shape(e.to_string()))?;
        let mut hooks = Self::default();
        for r in raw.into_iter().filter(|r| r.enabled) {
            let secret =
                SigningSecret::parse(&r.secret).ok_or_else(|| EndpointsError::Secret(r.id.clone()))?;
            let e = Endpoint { key: r.id, organization_id: r.organization_id, url: r.url, secret };
            if hooks.by_key.insert(e.key.clone(), e.clone()).is_some() {
                return Err(EndpointsError::Duplicate(e.key));
            }
            hooks.by_org.entry(e.organization_id.clone()).or_default().push(e);
        }
        Ok(hooks)
    }

    /// Whether any endpoint is configured.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// The endpoint keyed `key`, when it is still configured.
    pub fn endpoint(&self, key: &str) -> Option<&Endpoint> {
        self.by_key.get(key)
    }

    /// One delivery of `kind` about `id` to each of `org`'s endpoints, all
    /// with one event id and one body.
    pub fn deliveries(&self, org: &str, workspace: &str, kind: EventKind, id: &str) -> Vec<Delivery> {
        let Some(endpoints) = self.by_org.get(org) else { return Vec::new() };
        let (webhook_id, body) = body(org, workspace, &kind, id);
        endpoints
            .iter()
            .map(|e| Delivery {
                webhook_id: webhook_id.clone(),
                endpoint_key: e.key.clone(),
                endpoint_url: e.url.clone(),
                body: body.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_read_by_organization_and_disabled_ones_are_left_out() {
        let json = r#"[
            {"id": "a", "organization_id": "org_1", "url": "https://a.test/hook", "secret": "whsec_c2VjcmV0"},
            {"id": "b", "organization_id": "org_1", "url": "https://b.test/hook", "secret": "whsec_c2VjcmV0", "enabled": false}
        ]"#;
        let hooks = Webhooks::parse(json).unwrap();
        let sent = hooks.deliveries("org_1", "wrkspc_1", EventKind::SessionIdled, "sesn_1");
        assert_eq!(sent.len(), 1);
        assert_eq!(
            (sent[0].endpoint_key.as_str(), sent[0].endpoint_url.as_str()),
            ("a", "https://a.test/hook")
        );
        assert!(hooks.deliveries("org_2", "wrkspc_1", EventKind::SessionIdled, "sesn_1").is_empty());
        assert!(matches!(
            Webhooks::parse(r#"[{"id":"a","organization_id":"o","url":"u","secret":"nope!"}]"#),
            Err(EndpointsError::Secret(_))
        ));
        assert!(Webhooks::parse("{}").is_err());
    }
}
