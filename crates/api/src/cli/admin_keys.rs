//! `agentd admin-keys create|list|revoke`: organization admin keys, one
//! Secrets Manager secret each. An admin key reaches only the
//! `/v1/organizations` routes.

use std::process::ExitCode;
use std::sync::Arc;

use secrets_aws::{AdminKeyRecord, AwsAdminKeys, AwsSecretStore};
use shared::ports::secrets::ExposeSecret;

use super::{KeysCommand, USAGE};
use crate::config::Config;

/// Run `args` (the words after `agentd admin-keys`) against `config`'s Secrets Manager.
pub async fn admin_keys(config: &Config, args: &[String]) -> ExitCode {
    let command = match KeysCommand::parse(args, true) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match run(config, command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentd admin-keys: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: &Config, command: KeysCommand) -> Result<(), String> {
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    let pepper = secrets.api_key_pepper().await.map_err(|e| e.to_string())?;
    let keys = AwsAdminKeys::new(secrets, Arc::new(pepper));
    match command {
        KeysCommand::Create { org, name, .. } => {
            let (record, key) = keys.create(&org, &name).await.map_err(|e| e.to_string())?;
            println!("{}", key.expose_secret());
            eprintln!("created {}; the key above is shown once and cannot be read again", line(&record));
        }
        KeysCommand::List => {
            for record in keys.list().await.map_err(|e| e.to_string())? {
                println!("{}", line(&record));
            }
        }
        KeysCommand::Revoke(id) => {
            if !keys.revoke(&id).await.map_err(|e| e.to_string())? {
                return Err(format!("no active admin key {id}"));
            }
            eprintln!("revoked {id}");
        }
    }
    Ok(())
}

/// `id org status hint created name`, tab separated.
fn line(r: &AdminKeyRecord) -> String {
    let status = if r.revoked { "revoked" } else { "active" };
    [r.id.as_str(), &r.org, status, &r.partial_key_hint, &r.created_at, &r.name].join("\t")
}
