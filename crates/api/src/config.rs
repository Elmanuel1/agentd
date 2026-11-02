//! Boot configuration: which backend fills each port, and how to reach it.
//! The binary reads it from its environment once, through
//! [`Config::from_lookup`]; nothing else in agentd reads the environment.

use blobs_s3::S3Config;
use store_postgres::PoolSettings;

use crate::keys::env_keys;
pub use crate::keys::{ApiKey, DEV_KEY};

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
        /// `AGENTD_DB_POOL_SIZE` connections (default 10), each caller waiting
        /// at most `AGENTD_DB_POOL_WAIT_MS` (default 5000) for one.
        pool: PoolSettings,
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

/// `AGENTD_SCHEDULER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerConfig {
    /// `local`: the in-process scheduler (single replica).
    Local,
}

/// `AGENTD_RUNTIME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeConfig {
    /// `none`: no sandbox; a launch fails.
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

impl Config {
    /// Read the configuration through `get`, which returns a variable's value.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let store = match selector(&get, "AGENTD_STORE", "postgres").as_str() {
            "postgres" => StoreConfig::Postgres {
                url: get("DATABASE_URL")
                    .unwrap_or_else(|| "postgres://agentd:agentd@localhost:5432/agentd".into()),
                pool: PoolSettings {
                    max_size: number(&get, "AGENTD_DB_POOL_SIZE", 10)?,
                    wait: std::time::Duration::from_millis(number(&get, "AGENTD_DB_POOL_WAIT_MS", 5000)?),
                },
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

/// A positive whole number from `var`, or `default` when it is unset.
fn number<T: std::str::FromStr + PartialOrd + From<u8>>(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    default: T,
) -> Result<T, ConfigError> {
    match get(var) {
        None => Ok(default),
        Some(v) => v
            .parse::<T>()
            .ok()
            .filter(|n| *n > T::from(0))
            .ok_or_else(|| ConfigError { var, reason: format!("`{v}` is not a positive whole number") }),
    }
}

fn selector(get: &impl Fn(&str) -> Option<String>, var: &str, default: &str) -> String {
    get(var).unwrap_or_else(|| default.to_owned())
}

fn unknown(var: &'static str, value: &str) -> ConfigError {
    ConfigError { var, reason: format!("unknown backend `{value}`") }
}

pub(crate) fn flag(
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
        location_constraint: get("AGENTD_S3_LOCATION_CONSTRAINT"),
    })
}

#[cfg(test)]
mod tests;
