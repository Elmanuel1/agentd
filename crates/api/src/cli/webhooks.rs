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

    const LONG: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    /// Nothing listens there: a command that reached the database would fail differently.
    const NO_DATABASE: &str = "postgres://agentd@127.0.0.1:1/agentd";

    fn config() -> Config {
        configured(&[("ep_1", LONG)], NO_DATABASE).unwrap()
    }

    /// The configuration with `endpoints` (id and secret) of `org_1`, over `database`.
    fn configured(endpoints: &[(&str, &str)], database: &str) -> Result<Config, crate::config::ConfigError> {
        let listed: Vec<serde_json::Value> = endpoints
            .iter()
            .map(|(id, secret)| {
                serde_json::json!({"id": id, "organization_id": "org_1", "url": "https://hooks.example.com/a", "secret": secret})
            })
            .collect();
        let endpoints = serde_json::Value::Array(listed).to_string();
        let all = [
            ("DATABASE_URL", database),
            ("AGENTD_API_BASE_URL", "http://agentd:8080"),
            ("AGENTD_VALKEY_URL", "redis://127.0.0.1:1"),
            ("AGENTD_WORK_TOKEN_KEYS", "k1:000102030405060708090a0b0c0d0e0f"),
            ("AGENTD_PLACEHOLDER_KEYS", "k1:101112131415161718191a1b1c1d1e1f"),
            ("AGENTD_API_KEY_PEPPERS", "p1:202122232425262728292a2b2c2d2e2f"),
            ("AGENTD_SECRET_KEYS", "k1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
            ("AGENTD_WEBHOOK_ENDPOINTS", endpoints.as_str()),
        ];
        Config::from_lookup(|k| all.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned()))
    }

    #[test]
    fn a_webhook_secret_under_24_bytes_stops_the_boot_and_names_the_endpoint() {
        // SH-49: "secret" decodes to 6 bytes.
        let refused = configured(&[("ep_1", "whsec_c2VjcmV0")], NO_DATABASE).unwrap_err();
        assert_eq!(refused.var, "AGENTD_WEBHOOK_ENDPOINTS");
        assert_eq!(refused.reason, "endpoint ep_1: the secret decodes to fewer than 24 bytes");
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

    #[tokio::test]
    async fn enable_succeeds_for_a_disabled_endpoint_and_fails_for_one_never_sent_to() {
        use ::webhooks::states::{DisabledReason, EndpointStates};
        if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
            return;
        }
        let database = std::env::var("DATABASE_URL").unwrap();
        let (sent, never) = (
            format!("ep_{}", uuid::Uuid::new_v4().simple()),
            format!("ep_{}", uuid::Uuid::new_v4().simple()),
        );
        let config = configured(&[(sent.as_str(), LONG), (never.as_str(), LONG)], &database).unwrap();
        let store = PgWebhooks::new(store_postgres::PgBackend::connect(&database).await.unwrap());
        let endpoint = config.webhooks.endpoint(&sent).unwrap().configured();
        store.configure(std::slice::from_ref(&endpoint)).await.unwrap();
        store.disable(&endpoint, DisabledReason::Redirect).await.unwrap();
        assert_eq!(webhooks(&config, &words(&["enable", "org_1", sent.as_str()])).await, ExitCode::SUCCESS);
        assert!(!store.read().await.unwrap().iter().any(|s| s.endpoint_key == sent), "enabled");
        assert_eq!(webhooks(&config, &words(&["enable", "org_1", never.as_str()])).await, ExitCode::FAILURE);
    }
}
