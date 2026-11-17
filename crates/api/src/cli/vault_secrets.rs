//! `agentd vault-secrets tag|reseal`.
//!
//! `tag` tags every `environment_variable` credential's Secrets Manager
//! secret `credential_type=environment_variable`, so that the egress swap's
//! AWS policy reaches those secrets and no others. Secrets created since
//! carry the tag already; tagging one twice changes nothing.
//!
//! `reseal` seals every Postgres row not under the first key of
//! `AGENTD_SECRET_KEYS` again, so that an older master key can be dropped.
//!
//! Neither has anything to do on the other stores: Postgres and Parameter
//! Store record the kind when they write, and the AWS stores' KMS key does
//! their encryption.

use std::process::ExitCode;
use std::sync::Arc;

use secrets_aws::{AwsSecretStore, AwsSecretsConfig};
use store_postgres::{PgBackend, PgSecretStore, PgVaultsStore, Resealed};

use super::command::as_operator;
use crate::config::{Config, SecretsConfig, StoreConfig};

/// How to call it.
pub const USAGE: &str = "usage: agentd vault-secrets tag|reseal";

/// Run `args` (the words after `agentd vault-secrets`) against `config`'s backends.
pub async fn vault_secrets(config: &Config, args: &[String]) -> ExitCode {
    let (lines, code) = outcome(config, args).await;
    for line in lines {
        eprintln!("{line}");
    }
    code
}

/// What the command prints, and how it exits.
async fn outcome(config: &Config, args: &[String]) -> (Vec<String>, ExitCode) {
    let cmd = match args {
        [cmd] => cmd.as_str(),
        _ => "",
    };
    let error = |e: String| failed(format!("agentd vault-secrets: {e}"));
    match (cmd, &config.secrets) {
        ("tag", SecretsConfig::Aws(aws)) => as_operator(tag(config, aws)).await.map_or_else(error, |n| {
            (vec![format!("tagged {n} environment_variable secrets")], ExitCode::SUCCESS)
        }),
        ("tag", _) => (vec![NOTHING_TO_TAG.into()], ExitCode::SUCCESS),
        ("reseal", SecretsConfig::Postgres(keys)) => {
            let StoreConfig::Postgres { url, .. } = &config.store;
            let resealed = async {
                let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
                PgSecretStore::new(pg, keys.clone()).reseal().await.map_err(|e| e.to_string())
            };
            resealed.await.map_or_else(error, |r| report(&r))
        }
        ("reseal", _) => (vec![NOTHING_TO_RESEAL.into()], ExitCode::SUCCESS),
        _ => failed(format!("unknown vault-secrets command\n{USAGE}")),
    }
}

const NOTHING_TO_TAG: &str = "nothing to tag: this store records the kind when it writes";
const NOTHING_TO_RESEAL: &str = "nothing to reseal: the store's KMS key does the encryption";

fn failed(line: String) -> (Vec<String>, ExitCode) {
    (vec![line], ExitCode::FAILURE)
}

async fn tag(config: &Config, aws: &AwsSecretsConfig) -> Result<usize, String> {
    let secrets = AwsSecretStore::connect(aws).await.map_err(|e| e.to_string())?;
    let StoreConfig::Postgres { url, .. } = &config.store;
    let pg = PgBackend::connect(url).await.map_err(|e| e.to_string())?;
    let vaults = PgVaultsStore::new(pg, Arc::new(secrets.clone()));
    let keys = vaults.environment_variable_secrets().await.map_err(|e| e.to_string())?;
    for key in &keys {
        secrets.tag_environment_variable(key).await.map_err(|e| e.to_string())?;
    }
    Ok(keys.len())
}

/// Each row still under another key, then the counts; a failure while any
/// row remains, since its key cannot be dropped yet.
fn report(resealed: &Resealed) -> (Vec<String>, ExitCode) {
    let mut lines: Vec<String> = resealed
        .remaining
        .iter()
        .map(|(credential, key)| format!("still under key {key}: {credential}"))
        .collect();
    let (moved, left) = (resealed.moved, resealed.remaining.len());
    lines.push(format!("resealed {moved} vault secrets; {left} remain under another key"));
    if left > 0 {
        return (lines, ExitCode::FAILURE);
    }
    (lines, ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Config {
        let mut all = vec![
            ("DATABASE_URL", "postgres://agentd@127.0.0.1:1/agentd"),
            ("AGENTD_API_BASE_URL", "http://agentd:8080"),
            ("AGENTD_VALKEY_URL", "redis://127.0.0.1:1"),
            ("AGENTD_WORK_TOKEN_KEYS", "k1:000102030405060708090a0b0c0d0e0f"),
            ("AGENTD_PLACEHOLDER_KEYS", "k1:101112131415161718191a1b1c1d1e1f"),
            ("AGENTD_API_KEY_PEPPERS", "p1:202122232425262728292a2b2c2d2e2f"),
            ("AGENTD_SECRET_KEYS", "k1:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"),
        ];
        all.extend_from_slice(vars);
        Config::from_lookup(|k| all.iter().rev().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned()))
            .unwrap()
    }

    fn words(cmd: &str) -> Vec<String> {
        vec![cmd.to_owned()]
    }

    #[tokio::test]
    async fn tag_and_reseal_change_nothing_where_the_store_needs_neither_and_say_why() {
        // Nothing listens on the database or Valkey addresses: a command that
        // connected anything would fail.
        let postgres = config(&[]);
        let aws = config(&[("AGENTD_SECRETS", "aws"), ("AGENTD_SECRETS_KMS_KEY_ID", "alias/k")]);
        let ssm = config(&[("AGENTD_SECRETS", "ssm"), ("AGENTD_SECRETS_KMS_KEY_ID", "alias/k")]);
        let said = |line: &str| (vec![line.to_owned()], ExitCode::SUCCESS);
        let tag = "nothing to tag: this store records the kind when it writes";
        let reseal = "nothing to reseal: the store's KMS key does the encryption";
        assert_eq!(outcome(&postgres, &words("tag")).await, said(tag));
        assert_eq!(outcome(&ssm, &words("tag")).await, said(tag));
        assert_eq!(outcome(&aws, &words("reseal")).await, said(reseal));
        assert_eq!(outcome(&ssm, &words("reseal")).await, said(reseal));
        let (_, code) = outcome(&postgres, &words("rotate")).await;
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn reseal_names_each_remaining_row_and_fails_while_any_remains() {
        let done = Resealed { moved: 3, remaining: vec![] };
        let lines = vec!["resealed 3 vault secrets; 0 remain under another key".to_owned()];
        assert_eq!(report(&done), (lines, ExitCode::SUCCESS));
        let left = Resealed { moved: 2, remaining: vec![("wrkspc_a/vlt_b/cred_c".into(), "k0".into())] };
        let lines = vec![
            "still under key k0: wrkspc_a/vlt_b/cred_c".to_owned(),
            "resealed 2 vault secrets; 1 remain under another key".to_owned(),
        ];
        assert_eq!(report(&left), (lines, ExitCode::FAILURE));
    }
}
