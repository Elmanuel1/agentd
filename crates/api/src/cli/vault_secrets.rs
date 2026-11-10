//! `agentd vault-secrets tag`: tag every `environment_variable` credential's
//! secret `credential_type=environment_variable`, so that the egress swap's
//! AWS policy reaches those secrets and no others. Secrets created since
//! carry the tag already; tagging one twice changes nothing.

use std::process::ExitCode;
use std::sync::Arc;

use secrets_aws::AwsSecretStore;
use store_postgres::{PgBackend, PgVaultsStore};

use super::command::as_operator;
use crate::config::{Config, StoreConfig};

/// How to call it.
pub const USAGE: &str = "usage: agentd vault-secrets tag";

/// Run `args` (the words after `agentd vault-secrets`) against `config`'s backends.
pub async fn vault_secrets(config: &Config, args: &[String]) -> ExitCode {
    if !matches!(args, [cmd] if cmd == "tag") {
        eprintln!("unknown vault-secrets command\n{USAGE}");
        return ExitCode::FAILURE;
    }
    match as_operator(tag(config)).await {
        Ok(tagged) => {
            eprintln!("tagged {tagged} environment_variable secrets");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("agentd vault-secrets: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn tag(config: &Config) -> Result<usize, String> {
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    let StoreConfig::Postgres { url, .. } = &config.store;
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    let vaults = PgVaultsStore::new(pg, Arc::new(secrets.clone()));
    let keys = vaults.environment_variable_secrets().await.map_err(|e| e.to_string())?;
    for key in &keys {
        secrets.tag_environment_variable(key).await.map_err(|e| e.to_string())?;
    }
    Ok(keys.len())
}
