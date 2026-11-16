//! `agentd admin-keys create|list|revoke`: organization admin keys, in
//! Postgres, digested with the same peppers as workspace keys. An admin key
//! reaches only the `/v1/organizations` routes.

use std::process::ExitCode;
use std::sync::Arc;

use api_keys::{AdminKey, AdminKeysService};
use shared::ports::secrets::ExposeSecret;
use store_postgres::PgAdminKeysStore;

use super::{KeysCommand, Kind, USAGE};
use crate::config::Config;

/// Run `args` (the words after `agentd admin-keys`) against `config`'s backends.
pub async fn admin_keys(config: &Config, args: &[String]) -> ExitCode {
    super::command::run("admin-keys", USAGE, Kind::Admin, args, |c| run(config, c)).await
}

async fn run(config: &Config, command: KeysCommand) -> Result<(), String> {
    let (pg, peppers) = super::backend(config).await?;
    let keys = AdminKeysService::new(Arc::new(PgAdminKeysStore::new(pg)), peppers);
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
fn line(r: &AdminKey) -> String {
    let status = if r.revoked_at.is_some() { "revoked" } else { "active" };
    let created = r.created_at.to_wire();
    [r.id.as_str(), r.org.as_str(), status, &r.partial_key_hint, &created, &r.name].join("\t")
}
