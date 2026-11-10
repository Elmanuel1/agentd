//! `agentd keys create|list|revoke`: workspace keys, in Postgres.
//! `agentd admin-keys create|list|revoke`: admin keys, in Secrets Manager
//! ([`admin_keys`]). Both digest keys with the pepper in Secrets Manager. A
//! created key is printed once, alone on stdout, and is never stored;
//! everything else goes to stderr.

pub mod admin_keys;
mod command;
pub mod environment_keys;
pub mod vault_secrets;

use std::process::ExitCode;
use std::sync::Arc;

use api_keys::{ApiKey, ApiKeysService};
use secrets_aws::AwsSecretStore;
use shared::ports::secrets::ExposeSecret;
use store_postgres::{PgApiKeysStore, PgBackend};

pub use self::command::{KeysCommand, Kind};
use crate::config::{Config, StoreConfig};

/// How to call it.
pub const USAGE: &str = "usage: agentd keys create --org <org_id> --workspace <workspace_id> [--name <name>]
       agentd keys list
       agentd keys revoke <apikey_id>
       agentd admin-keys create --org <org_id> [--name <name>]
       agentd admin-keys list
       agentd admin-keys revoke <adminkey_id>";

/// Run `args` (the words after `agentd keys`) against `config`'s backends.
pub async fn keys(config: &Config, args: &[String]) -> ExitCode {
    command::run("keys", USAGE, Kind::Workspace, args, |c| run(config, c)).await
}

async fn run(config: &Config, command: KeysCommand) -> Result<(), String> {
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    let pepper = secrets.api_key_pepper().await.map_err(|e| e.to_string())?;
    let StoreConfig::Postgres { url, .. } = &config.store;
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    let keys = ApiKeysService::new(Arc::new(PgApiKeysStore::new(pg)), Arc::new(pepper));
    match command {
        KeysCommand::Create { org, workspace, name, .. } => {
            let workspace = workspace.unwrap_or_default();
            let (record, key) = keys.create(&org, &workspace, &name).await.map_err(|e| e.to_string())?;
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

/// `id org workspace status hint created name`, tab separated.
fn line(r: &ApiKey) -> String {
    let status = if r.revoked_at.is_some() { "revoked" } else { "active" };
    let created = r.created_at.to_wire();
    [r.id.as_str(), r.org.as_str(), r.workspace.as_str(), status, &r.partial_key_hint, &created, &r.name]
        .join("\t")
}
