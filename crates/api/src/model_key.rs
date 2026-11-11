//! `agentd model-key set [--provider anthropic|openrouter] [--host <host>]`:
//! a model provider's key, an operator credential. It is read from stdin,
//! stored in Secrets Manager as an environment-variable credential bound to
//! the provider's host, and never printed. The loop reads it to call the
//! model; the egress service puts OpenRouter's into a sandbox's model
//! requests, and the sandbox sees only a placeholder.

use std::io::Read as _;
use std::process::ExitCode;

use secrets_aws::AwsSecretStore;
use shared::ports::secrets::{SecretError, SecretKey, SecretStore};
use vaults::types::{CredentialAuthInput, CredentialNetworking, InjectionLocation, SecretString};

use engine::model::ModelProvider;

use crate::config::Config;

/// A model provider, its key stored at `PREFIX/operator/models/<name>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// Its name in the key's path and in `--provider`.
    pub name: &'static str,
    /// The variable a sandbox's harness reads its key from (as a placeholder).
    pub variable: &'static str,
    /// The host its key is sent to unless `--host` names another.
    pub host: &'static str,
}

/// OpenRouter, an OpenAI-compatible API, the default.
pub const OPENROUTER: Provider = Provider {
    name: ModelProvider::OpenAiCompat.name(),
    variable: "OPENROUTER_API_KEY",
    host: "openrouter.ai",
};
/// The Anthropic Messages API.
pub const ANTHROPIC: Provider = Provider {
    name: ModelProvider::Anthropic.name(),
    variable: "ANTHROPIC_API_KEY",
    host: "api.anthropic.com",
};

/// Where OpenRouter's key is stored, which the egress service also reads.
pub const KEY: (&str, &str, &str) = ("operator", "models", OPENROUTER.name);
/// The variable the sandbox's harness reads it from (as a placeholder).
pub const VARIABLE: &str = OPENROUTER.variable;

/// How to call it.
pub const USAGE: &str = "usage: agentd model-key set [--provider anthropic|openrouter] [--host <host>] < key";

/// Run `args` (the words after `agentd model-key`).
pub async fn model_key(config: &Config, args: &[String]) -> ExitCode {
    let Some((provider, host)) = parse(args) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    match store(config, provider, &host).await {
        Ok(()) => {
            eprintln!("stored the {} model key for {host}", provider.name);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("agentd model-key: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Option<(Provider, String)> {
    let (first, rest) = args.split_first()?;
    let (mut provider, mut host) = (OPENROUTER, None);
    let mut flags = rest.chunks(2);
    (first == "set").then_some(())?;
    for pair in flags.by_ref() {
        match pair {
            [flag, name] if flag == "--provider" => {
                provider = [OPENROUTER, ANTHROPIC].into_iter().find(|p| p.name == name)?;
            }
            [flag, h] if flag == "--host" && !h.is_empty() => host = Some(h.clone()),
            _ => return None,
        }
    }
    Some((provider, host.unwrap_or_else(|| provider.host.to_owned())))
}

/// The key stored for `provider`, if one is.
pub async fn read(secrets: &dyn SecretStore, provider: Provider) -> Result<Option<String>, String> {
    let key = SecretKey::new(KEY.0, KEY.1, provider.name).map_err(|e| e.to_string())?;
    let sealed = match secrets.get(&key).await {
        Ok(sealed) => sealed,
        Err(SecretError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    match CredentialAuthInput::unseal(&sealed).map_err(|e| e.to_string())? {
        CredentialAuthInput::EnvironmentVariable { secret_value, .. } => {
            Ok(Some(secret_value.expose().to_owned()))
        }
        _ => Err(format!("the {} model key is not an environment-variable credential", provider.name)),
    }
}

async fn store(config: &Config, provider: Provider, host: &str) -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let value = SecretString::new(raw.trim());
    raw.clear();
    if value.expose().is_empty() {
        return Err("no key on stdin".into());
    }
    let sealed = CredentialAuthInput::EnvironmentVariable {
        secret_name: provider.variable.into(),
        secret_value: value,
        networking: Some(CredentialNetworking::Limited { allowed_hosts: vec![host.to_owned()] }),
        injection_location: Some(InjectionLocation { body: false, header: true }),
    }
    .seal();
    let key = SecretKey::new(KEY.0, KEY.1, provider.name).map_err(|e| e.to_string())?;
    let secrets = AwsSecretStore::connect(&config.secrets).await.map_err(|e| e.to_string())?;
    secrets.put(&key, &sealed).await.map_err(|e| e.to_string())
}
