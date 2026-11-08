//! `agentd keys create|list|revoke`: workspace keys, in Postgres.
//! `agentd admin-keys create|list|revoke`: admin keys, in Secrets Manager
//! ([`admin_keys`]). Both digest keys with the pepper in Secrets Manager. A
//! created key is printed once, alone on stdout, and is never stored;
//! everything else goes to stderr.

pub mod admin_keys;

use std::process::ExitCode;
use std::sync::Arc;

use api_keys::{ApiKey, ApiKeysService};
use secrets_aws::AwsSecretStore;
use shared::ports::secrets::ExposeSecret;
use shared::{with_writer, Writer};
use store_postgres::{PgApiKeysStore, PgBackend};

use crate::config::{Config, StoreConfig};

/// How to call it.
pub const USAGE: &str = "usage: agentd keys create --org <org_id> --workspace <workspace_id> [--name <name>]
       agentd keys list
       agentd keys revoke <apikey_id>
       agentd admin-keys create --org <org_id> [--name <name>]
       agentd admin-keys list
       agentd admin-keys revoke <adminkey_id>";

/// One `agentd keys` or `agentd admin-keys` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysCommand {
    /// Issue a key for an org and, for a workspace key, a workspace.
    Create {
        /// The org id.
        org: String,
        /// The workspace id; `None` for an admin key.
        workspace: Option<String>,
        /// A label for people.
        name: String,
    },
    /// Show every key, without its secret.
    List,
    /// Revoke a key by its `apikey_…` id.
    Revoke(String),
}

impl KeysCommand {
    /// The command in `args`, the words after `agentd keys` (`admin` false)
    /// or `agentd admin-keys` (`admin` true, which takes no workspace).
    pub fn parse(args: &[String], admin: bool) -> Result<Self, String> {
        match args {
            [cmd, rest @ ..] if cmd == "create" => {
                let (mut org, mut workspace, mut name) = (None, None, None);
                let mut flags = rest.iter();
                while let Some(flag) = flags.next() {
                    let slot = match flag.as_str() {
                        "--org" => &mut org,
                        "--workspace" if !admin => &mut workspace,
                        "--name" => &mut name,
                        other => return Err(format!("unknown option `{other}`")),
                    };
                    *slot = Some(flags.next().ok_or_else(|| format!("{flag} needs a value"))?.clone());
                }
                if !admin && workspace.is_none() {
                    return Err("--workspace is required".into());
                }
                Ok(Self::Create {
                    org: org.ok_or("--org is required")?,
                    workspace,
                    name: name.unwrap_or_else(|| "agentd".into()),
                })
            }
            [cmd] if cmd == "list" => Ok(Self::List),
            [cmd, id] if cmd == "revoke" => Ok(Self::Revoke(id.clone())),
            _ => Err("unknown keys command".into()),
        }
    }
}

/// Run `args` (the words after `agentd keys`) against `config`'s backends.
pub async fn keys(config: &Config, args: &[String]) -> ExitCode {
    let command = match KeysCommand::parse(args, false) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match as_operator(run(config, command)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentd keys: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Run `fut` as the CLI's writer, `operator`, in a request of its own.
async fn as_operator<F: std::future::Future>(fut: F) -> F::Output {
    with_writer(Writer::fresh("operator"), fut).await
}

async fn run(config: &Config, command: KeysCommand) -> Result<(), String> {
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    let pepper = secrets.api_key_pepper().await.map_err(|e| e.to_string())?;
    let StoreConfig::Postgres { url, .. } = &config.store;
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    let keys = ApiKeysService::new(Arc::new(PgApiKeysStore::new(pg)), Arc::new(pepper));
    match command {
        KeysCommand::Create { org, workspace, name } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Vec<String> {
        words.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn create_takes_an_org_a_workspace_and_an_optional_name() {
        assert_eq!(
            KeysCommand::parse(&args("create --workspace wrkspc_a --org org_a"), false),
            Ok(KeysCommand::Create {
                org: "org_a".into(),
                workspace: Some("wrkspc_a".into()),
                name: "agentd".into()
            })
        );
        assert!(matches!(
            KeysCommand::parse(&args("create --org o --workspace w --name ci"), false),
            Ok(KeysCommand::Create { name, .. }) if name == "ci"
        ));
        for bad in ["create", "create --org o", "create --org o --workspace", "create --org o --key k"] {
            assert!(KeysCommand::parse(&args(bad), false).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_admin_key_takes_an_org_and_no_workspace() {
        assert_eq!(
            KeysCommand::parse(&args("create --org org_a"), true),
            Ok(KeysCommand::Create { org: "org_a".into(), workspace: None, name: "agentd".into() })
        );
        for bad in ["create", "create --org o --workspace w"] {
            assert!(KeysCommand::parse(&args(bad), true).is_err(), "{bad}");
        }
    }

    #[test]
    fn list_and_revoke_take_what_they_need() {
        for admin in [false, true] {
            assert_eq!(KeysCommand::parse(&args("list"), admin), Ok(KeysCommand::List));
            assert_eq!(
                KeysCommand::parse(&args("revoke apikey_01a"), admin),
                Ok(KeysCommand::Revoke("apikey_01a".into()))
            );
            for bad in ["", "list x", "revoke", "revoke a b", "rotate"] {
                assert!(KeysCommand::parse(&args(bad), admin).is_err(), "{bad}");
            }
        }
    }
}
