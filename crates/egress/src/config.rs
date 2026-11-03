//! The service's settings, from the environment.

use secrecy::SecretString;

/// Shortest admin token accepted: 32 characters.
const MIN_TOKEN: usize = 32;

/// agentd-egress settings.
#[derive(Debug)]
pub struct Config {
    /// `AGENTD_EGRESS_ADDR` (default `0.0.0.0:8890`): the proxy, for sandboxes.
    pub addr: String,
    /// `AGENTD_EGRESS_ADMIN_ADDR` (default `127.0.0.1:8891`): the admin
    /// interface, for agentd only.
    pub admin_addr: String,
    /// `AGENTD_EGRESS_ADMIN_TOKEN` (required, at least 32 characters).
    pub admin_token: SecretString,
    /// `AGENTD_EGRESS_INTERNAL_HOSTS`: hosts sandboxes may reach at internal
    /// addresses, comma-separated.
    pub internal_hosts: Vec<String>,
    /// `AGENTD_SECRETS_PREFIX` (default `agentd`).
    pub prefix: String,
    /// `AGENTD_SECRETS_REGION`.
    pub region: Option<String>,
    /// `AGENTD_SECRETS_ENDPOINT` (LocalStack).
    pub endpoint: Option<String>,
    /// `AGENTD_SECRETS_ROLE_ARN`.
    pub role_arn: Option<String>,
}

impl Config {
    /// Read the settings through `get`.
    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let token = get("AGENTD_EGRESS_ADMIN_TOKEN").unwrap_or_default();
        if token.chars().count() < MIN_TOKEN {
            return Err(format!("AGENTD_EGRESS_ADMIN_TOKEN: required, at least {MIN_TOKEN} characters"));
        }
        let set = |k: &str| get(k).filter(|v| !v.trim().is_empty());
        Ok(Self {
            addr: set("AGENTD_EGRESS_ADDR").unwrap_or_else(|| "0.0.0.0:8890".into()),
            admin_addr: set("AGENTD_EGRESS_ADMIN_ADDR").unwrap_or_else(|| "127.0.0.1:8891".into()),
            admin_token: SecretString::from(token),
            internal_hosts: set("AGENTD_EGRESS_INTERNAL_HOSTS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(str::to_owned)
                .collect(),
            prefix: set("AGENTD_SECRETS_PREFIX").unwrap_or_else(|| secrets_aws::DEFAULT_PREFIX.into()),
            region: set("AGENTD_SECRETS_REGION"),
            endpoint: set("AGENTD_SECRETS_ENDPOINT"),
            role_arn: set("AGENTD_SECRETS_ROLE_ARN"),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn the_admin_token_is_required_and_long() {
        assert!(Config::from_lookup(&lookup(&[])).is_err());
        assert!(Config::from_lookup(&lookup(&[("AGENTD_EGRESS_ADMIN_TOKEN", "short")])).is_err());
        let c = Config::from_lookup(&lookup(&[("AGENTD_EGRESS_ADMIN_TOKEN", &"t".repeat(32))])).unwrap();
        assert_eq!(
            (c.addr.as_str(), c.admin_addr.as_str(), c.prefix.as_str()),
            ("0.0.0.0:8890", "127.0.0.1:8891", "agentd")
        );
        assert!(!format!("{c:?}").contains(&"t".repeat(32)));
    }
}
