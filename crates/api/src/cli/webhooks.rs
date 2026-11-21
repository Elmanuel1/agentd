//! `agentd webhooks enable <org> <endpoint>`: send to an endpoint the sender
//! disabled (a 3xx, a non-public address, or 120 h of failures) again, with
//! its run of failures cleared (D6). Changing the endpoint's URL or secret in
//! `AGENTD_WEBHOOK_ENDPOINTS` enables it too, at the next boot.

use std::process::ExitCode;

use store_postgres::PgWebhooks;

use super::command::as_operator;
use crate::config::Config;

/// How to call it.
pub const USAGE: &str = "usage: agentd webhooks enable <org_id> <endpoint_id>";

/// Run `args` (the words after `agentd webhooks`) against `config`'s database.
pub async fn webhooks(config: &Config, args: &[String]) -> ExitCode {
    let [cmd, org, endpoint] = args else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    if cmd != "enable" {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }
    match as_operator(enable(config, org, endpoint)).await {
        Ok(()) => {
            eprintln!("enabled {endpoint}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("agentd webhooks: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn enable(config: &Config, org: &str, endpoint: &str) -> Result<(), String> {
    if !config.webhooks.endpoint(endpoint).is_some_and(|e| e.organization_id == org) {
        return Err(format!("{org} has no endpoint {endpoint} in AGENTD_WEBHOOK_ENDPOINTS"));
    }
    let (pg, _) = super::backend(config).await?;
    match PgWebhooks::new(pg).enable(endpoint).await.map_err(|e| e.to_string())? {
        true => Ok(()),
        false => Err(format!("agentd has not sent to {endpoint} yet, so it is not disabled")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let endpoints = r#"[{"id": "ep_1", "organization_id": "org_1", "url": "https://hooks.example.com/a", "secret": "whsec_c2VjcmV0"}]"#;
        let all = [
            ("DATABASE_URL", "postgres://agentd@127.0.0.1:1/agentd"),
            ("AGENTD_API_BASE_URL", "http://agentd:8080"),
            ("AGENTD_VALKEY_URL", "redis://127.0.0.1:1"),
            ("AGENTD_WORK_TOKEN_KEYS", "k1:000102030405060708090a0b0c0d0e0f"),
            ("AGENTD_PLACEHOLDER_KEYS", "k1:101112131415161718191a1b1c1d1e1f"),
            ("AGENTD_API_KEY_PEPPERS", "p1:202122232425262728292a2b2c2d2e2f"),
            ("AGENTD_SECRET_KEYS", "k1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
            ("AGENTD_WEBHOOK_ENDPOINTS", endpoints),
        ];
        Config::from_lookup(|k| all.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())).unwrap()
    }

    fn words(all: &[&str]) -> Vec<String> {
        all.iter().map(|w| (*w).to_owned()).collect()
    }

    #[tokio::test]
    async fn a_wrong_command_fails_with_its_usage_and_reads_no_database() {
        // Nothing listens on the database address: a command that connected would fail differently.
        let config = config();
        for args in [words(&["enable", "org_1"]), words(&["disable", "org_1", "ep_1"]), words(&[])] {
            assert_eq!(webhooks(&config, &args).await, ExitCode::FAILURE, "{args:?}");
        }
    }

    #[tokio::test]
    async fn an_endpoint_outside_its_org_or_not_configured_is_refused_before_the_database() {
        let config = config();
        let not_configured = |org: &str, endpoint: &str| {
            format!("{org} has no endpoint {endpoint} in AGENTD_WEBHOOK_ENDPOINTS")
        };
        assert_eq!(enable(&config, "org_2", "ep_1").await.unwrap_err(), not_configured("org_2", "ep_1"));
        assert_eq!(enable(&config, "org_1", "ep_9").await.unwrap_err(), not_configured("org_1", "ep_9"));
        let read = enable(&config, "org_1", "ep_1").await.unwrap_err();
        assert_ne!(read, not_configured("org_1", "ep_1"), "a configured endpoint goes on to the database");
    }
}
