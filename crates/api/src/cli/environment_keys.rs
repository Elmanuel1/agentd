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

use super::as_operator;
use crate::config::{Config, StoreConfig};

/// How to call it.
pub const USAGE: &str =
    "usage: agentd environment-keys create --org <org_id> --workspace <workspace_id> --environment <env_id> [--name <name>]
       agentd environment-keys list
       agentd environment-keys revoke <envkey_id>";

/// One `agentd environment-keys` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Issue a key for an environment of a workspace.
    Create {
        /// The org id.
        org: String,
        /// The workspace id.
        workspace: String,
        /// The environment id.
        environment: String,
        /// A label for people.
        name: String,
    },
    /// Show every key, without its secret.
    List,
    /// Revoke a key by its `envkey_…` id.
    Revoke(String),
}

impl Command {
    /// The command in `args`, the words after `agentd environment-keys`.
    pub fn parse(args: &[String]) -> Result<Self, String> {
        match args {
            [cmd, rest @ ..] if cmd == "create" => {
                let (mut org, mut workspace, mut environment, mut name) = (None, None, None, None);
                let mut flags = rest.iter();
                while let Some(flag) = flags.next() {
                    let slot = match flag.as_str() {
                        "--org" => &mut org,
                        "--workspace" => &mut workspace,
                        "--environment" => &mut environment,
                        "--name" => &mut name,
                        other => return Err(format!("unknown option `{other}`")),
                    };
                    *slot = Some(flags.next().ok_or_else(|| format!("{flag} needs a value"))?.clone());
                }
                Ok(Self::Create {
                    org: org.ok_or("--org is required")?,
                    workspace: workspace.ok_or("--workspace is required")?,
                    environment: environment.ok_or("--environment is required")?,
                    name: name.unwrap_or_else(|| "agentd".into()),
                })
            }
            [cmd] if cmd == "list" => Ok(Self::List),
            [cmd, id] if cmd == "revoke" => Ok(Self::Revoke(id.clone())),
            _ => Err("unknown environment-keys command".into()),
        }
    }
}

/// Run `args` (the words after `agentd environment-keys`) against `config`'s backends.
pub async fn environment_keys(config: &Config, args: &[String]) -> ExitCode {
    let command = match Command::parse(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match as_operator(run(config, command)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentd environment-keys: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: &Config, command: Command) -> Result<(), String> {
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    let pepper = secrets.api_key_pepper().await.map_err(|e| e.to_string())?;
    let StoreConfig::Postgres { url, .. } = &config.store;
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    let keys = EnvironmentKeysService::new(Arc::new(PgEnvironmentKeysStore::new(pg)), Arc::new(pepper));
    match command {
        Command::Create { org, workspace, environment, name } => {
            let (record, key) =
                keys.create(&org, &workspace, &environment, &name).await.map_err(|e| e.to_string())?;
            println!("{}", key.expose_secret());
            eprintln!("created {}; the key above is shown once and cannot be read again", line(&record));
        }
        Command::List => {
            for record in keys.list().await.map_err(|e| e.to_string())? {
                println!("{}", line(&record));
            }
        }
        Command::Revoke(id) => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &str) -> Vec<String> {
        words.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn create_takes_an_org_a_workspace_an_environment_and_an_optional_name() {
        assert_eq!(
            Command::parse(&args("create --environment env_a --workspace wrkspc_a --org org_a")),
            Ok(Command::Create {
                org: "org_a".into(),
                workspace: "wrkspc_a".into(),
                environment: "env_a".into(),
                name: "agentd".into()
            })
        );
        for bad in
            ["create", "create --org o --workspace w", "create --org o --environment e", "create --x y"]
        {
            assert!(Command::parse(&args(bad)).is_err(), "{bad}");
        }
        assert_eq!(Command::parse(&args("list")), Ok(Command::List));
        assert_eq!(Command::parse(&args("revoke envkey_1")), Ok(Command::Revoke("envkey_1".into())));
        for bad in ["", "list x", "revoke", "rotate"] {
            assert!(Command::parse(&args(bad)).is_err(), "{bad}");
        }
    }
}
