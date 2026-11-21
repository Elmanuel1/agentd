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
    let (pg, _) = super::backend(config).await?;
    match PgWebhooks::new(pg).enable(org, endpoint).await.map_err(|e| e.to_string())? {
        true => Ok(()),
        false => Err(format!("{org} has no endpoint {endpoint} that agentd has sent to")),
    }
}
