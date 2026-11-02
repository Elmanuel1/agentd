//! Boot configuration: which backend fills each port, and how to reach it.
//! The binary reads it from its environment once, through
//! [`Config::from_lookup`]; nothing else in agentd reads the environment.

use blobs_s3::S3Config;

/// Everything the binary needs to boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Listen address (`AGENTD_ADDR`).
    pub addr: String,
    /// Record store (`AGENTD_STORE`).
    pub store: StoreConfig,
    /// Blob store (`AGENTD_BLOBS`).
    pub blobs: BlobsConfig,
    /// Secret store (`AGENTD_SECRETS`).
    pub secrets: SecretsConfig,
    /// API-key source (`AGENTD_KEYS`).
    pub keys: KeysConfig,
    /// Scheduler (`AGENTD_SCHEDULER`).
    pub scheduler: SchedulerConfig,
    /// Sandbox runtime (`AGENTD_RUNTIME`).
    pub runtime: RuntimeConfig,
}

/// `AGENTD_STORE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreConfig {
    /// `postgres`, at `DATABASE_URL`.
    Postgres {
        /// The connection URL.
        url: String,
    },
}

/// `AGENTD_BLOBS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobsConfig {
    /// `s3`.
    S3(S3Config),
}

/// `AGENTD_SECRETS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretsConfig {
    /// `blob`: secrets as objects in the blob store.
    Blob,
}

/// `AGENTD_KEYS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysConfig {
    /// `env`: the keys listed in `AGENTD_API_KEYS`, plus the development key
    /// when `AGENTD_DEV_KEY=1`.
    Env(Vec<ApiKey>),
}

/// One API key and the tenant it is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    /// The secret the caller sends.
    pub key: String,
    /// Its organization.
    pub org: String,
    /// Its workspace.
    pub workspace: String,
    /// Its `apikey_…` id, the audit actor of its writes.
    pub api_key_id: String,
}

/// `AGENTD_SCHEDULER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerConfig {
    /// `local`: the in-process scheduler (single replica).
    Local,
}

/// `AGENTD_RUNTIME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeConfig {
    /// `none`: no sandbox; launches are recorded in memory only.
    None,
}

/// A configuration value agentd cannot boot with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{var}: {reason}")]
pub struct ConfigError {
    /// The variable at fault.
    pub var: &'static str,
    /// What is wrong with it.
    pub reason: String,
}

/// The key `AGENTD_DEV_KEY=1` registers.
pub const DEV_KEY: &str = "agentd-dev-key:org_dev:wrkspc_01dev";

impl Config {
    /// Read the configuration through `get`, which returns a variable's value.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let store = match selector(&get, "AGENTD_STORE", "postgres").as_str() {
            "postgres" => StoreConfig::Postgres {
                url: get("DATABASE_URL")
                    .unwrap_or_else(|| "postgres://agentd:agentd@localhost:5432/agentd".into()),
            },
            other => return Err(unknown("AGENTD_STORE", other)),
        };
        let blobs = match selector(&get, "AGENTD_BLOBS", "s3").as_str() {
            "s3" => BlobsConfig::S3(s3_config(&get)?),
            other => return Err(unknown("AGENTD_BLOBS", other)),
        };
        let secrets = match selector(&get, "AGENTD_SECRETS", "blob").as_str() {
            "blob" => SecretsConfig::Blob,
            other => return Err(unknown("AGENTD_SECRETS", other)),
        };
        let keys = match selector(&get, "AGENTD_KEYS", "env").as_str() {
            "env" => KeysConfig::Env(env_keys(&get)?),
            other => return Err(unknown("AGENTD_KEYS", other)),
        };
        let scheduler = match selector(&get, "AGENTD_SCHEDULER", "local").as_str() {
            "local" => SchedulerConfig::Local,
            other => return Err(unknown("AGENTD_SCHEDULER", other)),
        };
        let runtime = match selector(&get, "AGENTD_RUNTIME", "none").as_str() {
            "none" => RuntimeConfig::None,
            other => return Err(unknown("AGENTD_RUNTIME", other)),
        };
        let addr = get("AGENTD_ADDR").unwrap_or_else(|| "0.0.0.0:8080".into());
        Ok(Self { addr, store, blobs, secrets, keys, scheduler, runtime })
    }
}

fn selector(get: &impl Fn(&str) -> Option<String>, var: &str, default: &str) -> String {
    get(var).unwrap_or_else(|| default.to_owned())
}

fn unknown(var: &'static str, value: &str) -> ConfigError {
    ConfigError { var, reason: format!("unknown backend `{value}`") }
}

fn flag(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    match get(var).as_deref() {
        None => Ok(default),
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        Some(other) => {
            Err(ConfigError { var, reason: format!("expected 1, 0, true or false, got `{other}`") })
        }
    }
}

fn s3_config(get: &impl Fn(&str) -> Option<String>) -> Result<S3Config, ConfigError> {
    let endpoint = get("AGENTD_AWS_ENDPOINT");
    Ok(S3Config {
        bucket: get("AGENTD_BLOB_BUCKET").unwrap_or_else(|| "agentd-artifacts".into()),
        region: get("AGENTD_S3_REGION"),
        path_style: flag(get, "AGENTD_S3_PATH_STYLE", endpoint.is_some())?,
        endpoint,
        role_arn: get("AGENTD_S3_ROLE_ARN"),
        create_bucket: flag(get, "AGENTD_S3_CREATE_BUCKET", false)?,
    })
}

/// `AGENTD_API_KEYS` is `key:org:workspace[,key:org:workspace…]`.
fn env_keys(get: &impl Fn(&str) -> Option<String>) -> Result<Vec<ApiKey>, ConfigError> {
    const VAR: &str = "AGENTD_API_KEYS";
    let mut triples: Vec<String> =
        get(VAR).map(|v| v.split(',').map(str::to_owned).collect()).unwrap_or_default();
    if flag(get, "AGENTD_DEV_KEY", false)? {
        triples.push(DEV_KEY.to_owned());
    }
    if triples.is_empty() {
        return Err(ConfigError {
            var: VAR,
            reason: "no API keys; set it, or AGENTD_DEV_KEY=1 in development".into(),
        });
    }
    triples
        .iter()
        .enumerate()
        .map(|(i, triple)| match triple.splitn(3, ':').collect::<Vec<_>>().as_slice() {
            [key, org, workspace] if !key.is_empty() && !org.is_empty() && !workspace.is_empty() => {
                Ok(ApiKey {
                    key: (*key).to_owned(),
                    org: (*org).to_owned(),
                    workspace: (*workspace).to_owned(),
                    api_key_id: format!("apikey_01local{i:010}"),
                })
            }
            _ => Err(ConfigError { var: VAR, reason: format!("entry {i} is not key:org:workspace") }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn config(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        Config::from_lookup(|k| vars.get(k).cloned())
    }

    #[test]
    fn defaults_select_postgres_s3_blob_env_local_none() {
        let c = config(&[("AGENTD_API_KEYS", "k:org_a:wrkspc_a")]).unwrap();
        assert!(matches!(c.store, StoreConfig::Postgres { .. }));
        assert_eq!(c.blobs, BlobsConfig::S3(S3Config::new("agentd-artifacts")));
        assert_eq!(
            (c.secrets, c.scheduler, c.runtime),
            (SecretsConfig::Blob, SchedulerConfig::Local, RuntimeConfig::None)
        );
        let KeysConfig::Env(keys) = c.keys;
        assert_eq!(keys.iter().map(|k| k.key.as_str()).collect::<Vec<_>>(), ["k"]);
    }

    #[test]
    fn an_unknown_backend_is_an_error() {
        let err = config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_STORE", "dynamodb")]).unwrap_err();
        assert_eq!(err.var, "AGENTD_STORE");
    }

    #[test]
    fn no_key_is_registered_without_the_dev_flag() {
        assert_eq!(config(&[]).unwrap_err().var, "AGENTD_API_KEYS");
        let c = config(&[("AGENTD_DEV_KEY", "1")]).unwrap();
        let KeysConfig::Env(keys) = c.keys;
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key, "agentd-dev-key");
    }

    #[test]
    fn a_malformed_key_entry_is_an_error() {
        let err = config(&[("AGENTD_API_KEYS", "k:o:w,broken")]).unwrap_err();
        assert_eq!(
            err,
            ConfigError { var: "AGENTD_API_KEYS", reason: "entry 1 is not key:org:workspace".into() }
        );
    }

    #[test]
    fn s3_options_come_from_config() {
        let c = config(&[
            ("AGENTD_API_KEYS", "k:o:w"),
            ("AGENTD_BLOB_BUCKET", "b"),
            ("AGENTD_S3_REGION", "eu-west-1"),
            ("AGENTD_AWS_ENDPOINT", "http://localhost:4566"),
            ("AGENTD_S3_ROLE_ARN", "arn:aws:iam::123456789012:role/agentd"),
            ("AGENTD_S3_CREATE_BUCKET", "true"),
        ])
        .unwrap();
        assert_eq!(
            c.blobs,
            BlobsConfig::S3(S3Config {
                bucket: "b".into(),
                region: Some("eu-west-1".into()),
                endpoint: Some("http://localhost:4566".into()),
                path_style: true,
                role_arn: Some("arn:aws:iam::123456789012:role/agentd".into()),
                create_bucket: true,
            })
        );
        let c = config(&[
            ("AGENTD_API_KEYS", "k:o:w"),
            ("AGENTD_AWS_ENDPOINT", "http://m"),
            ("AGENTD_S3_PATH_STYLE", "0"),
        ])
        .unwrap();
        assert!(matches!(c.blobs, BlobsConfig::S3(S3Config { path_style: false, .. })));
        assert_eq!(
            config(&[("AGENTD_API_KEYS", "k:o:w"), ("AGENTD_S3_CREATE_BUCKET", "yes")]).unwrap_err().var,
            "AGENTD_S3_CREATE_BUCKET"
        );
    }
}
