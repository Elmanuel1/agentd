//! `agentd model-key set [--host <host>]`: the model provider's key, an
//! operator credential. It is read from stdin, stored in Secrets Manager as
//! an environment-variable credential bound to the model host, and never
//! printed. The egress service puts it into a sandbox's model requests to
//! that host; the sandbox sees only a placeholder.

use std::io::Read as _;
use std::process::ExitCode;

use secrets_aws::AwsSecretStore;
use shared::ports::secrets::{SecretKey, SecretStore};
use vaults::types::{CredentialAuthInput, CredentialNetworking, InjectionLocation, SecretString};

use crate::config::Config;

/// Where the key is stored: `PREFIX/operator/models/openrouter`.
pub const KEY: (&str, &str, &str) = ("operator", "models", "openrouter");
/// The variable the sandbox's harness reads it from (as a placeholder).
pub const VARIABLE: &str = "OPENROUTER_API_KEY";
/// The host the key is sent to unless `--host` names another.
pub const DEFAULT_HOST: &str = "openrouter.ai";

/// How to call it.
pub const USAGE: &str = "usage: agentd model-key set [--host <host>] < key";

/// Run `args` (the words after `agentd model-key`).
pub async fn model_key(config: &Config, args: &[String]) -> ExitCode {
    let host = match args {
        [set] if set == "set" => DEFAULT_HOST.to_owned(),
        [set, flag, host] if set == "set" && flag == "--host" && !host.is_empty() => host.clone(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    match store(config, &host).await {
        Ok(()) => {
            eprintln!("stored the model key for {host}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("agentd model-key: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn store(config: &Config, host: &str) -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let value = SecretString::new(raw.trim());
    raw.clear();
    if value.expose().is_empty() {
        return Err("no key on stdin".into());
    }
    let sealed = CredentialAuthInput::EnvironmentVariable {
        secret_name: VARIABLE.into(),
        secret_value: value,
        networking: Some(CredentialNetworking::Limited { allowed_hosts: vec![host.to_owned()] }),
        injection_location: Some(InjectionLocation { body: false, header: true }),
    }
    .seal();
    let key = SecretKey::new(KEY.0, KEY.1, KEY.2).map_err(|e| e.to_string())?;
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    secrets.put(&key, &sealed).await.map_err(|e| e.to_string())
}
