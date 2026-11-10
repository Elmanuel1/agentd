//! `agentd environment-keys create|list|revoke`: a self-hosted environment's
//! keys, in Postgres, digested with the same pepper as workspace keys. A key
//! is a Bearer token that reaches its own environment's work and sessions
//! only; its worker polls with it. Live issues these in the Console.

use std::process::ExitCode;
use std::sync::Arc;

use api_keys::{EnvironmentKey, EnvironmentKeysService};
use secrets_aws::AwsSecretStore;
use shared::ports::secrets::ExposeSecret;
use store_postgres::{PgBackend, PgEnvironmentKeysStore};

use super::{KeysCommand, Kind};
use crate::config::{Config, StoreConfig};

/// How to call it.
pub const USAGE: &str =
    "usage: agentd environment-keys create --org <org_id> --workspace <workspace_id> --environment <env_id> [--name <name>]
       agentd environment-keys list
       agentd environment-keys revoke <envkey_id>";

/// Run `args` (the words after `agentd environment-keys`) against `config`'s backends.
pub async fn environment_keys(config: &Config, args: &[String]) -> ExitCode {
    super::command::run("environment-keys", USAGE, Kind::Environment, args, |c| run(config, c)).await
}

async fn run(config: &Config, command: KeysCommand) -> Result<(), String> {
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    let pepper = secrets.api_key_pepper().await.map_err(|e| e.to_string())?;
    let StoreConfig::Postgres { url, .. } = &config.store;
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    let keys = EnvironmentKeysService::new(Arc::new(PgEnvironmentKeysStore::new(pg)), Arc::new(pepper));
    match command {
        KeysCommand::Create { org, workspace, environment, name } => {
            let (workspace, environment) = (workspace.unwrap_or_default(), environment.unwrap_or_default());
            let (record, key) =
                keys.create(&org, &workspace, &environment, &name).await.map_err(|e| e.to_string())?;
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
                return Err(format!("no active key {id}"));
            }
            eprintln!("revoked {id}");
        }
    }
    Ok(())
}

/// `id org workspace environment status hint created name`, tab separated.
fn line(r: &EnvironmentKey) -> String {
    let status = match r.revoked_at {
        Some(_) => "revoked",
        None => "active",
    };
    let created = r.created_at.to_wire();
    [
        r.id.as_str(),
        r.org.as_str(),
        r.workspace.as_str(),
        &r.environment_id,
        status,
        &r.partial_key_hint,
        &created,
        &r.name,
    ]
    .join("\t")
}
