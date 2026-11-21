//! Boot configuration: which backend fills each port, and how to reach it.
//! The binary reads it from its environment once, through
//! [`Config::from_lookup`]; nothing else in agentd reads the environment.

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
    /// The peppers, runtime keys and model keys from the environment.
    pub keys: KeysConfig,
    /// Where logs, traces and metrics go.
    pub telemetry: TelemetryConfig,
    /// `AGENTD_MODELS`: model ids agents may name beyond the built-in
    /// Anthropic ones, comma-separated.
    pub extra_models: Vec<String>,
    /// How workers reach agentd.
    pub work: WorkConfig,
    /// Session streams across replicas, and event retention.
    pub streams: StreamsConfig,
    /// What this replica runs, and the loop's settings.
    pub agent_loop: LoopConfig,
    /// Each organization's webhook endpoints.
    pub webhooks: std::sync::Arc<::webhooks::Webhooks>,
    /// The scheduler sink (`AGENTD_SCHEDULER`).
    pub scheduler: SchedulerConfig,
    /// `AGENTD_REQUEST_TIMEOUT_MS` (60,000): how long a response may take to start; files take 15 min.
    pub request_timeout: std::time::Duration,
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
        let store = StoreConfig::from_lookup(&get)?;
        let blobs = blobs::from_lookup(&get)?;
        let secrets = secrets::from_lookup(&get)?;
        let keys = KeysConfig::from_lookup(&get)?;
        let addr = get("AGENTD_ADDR").unwrap_or_else(|| "0.0.0.0:8080".into());
        let telemetry = TelemetryConfig::from_lookup(&get)?;
        let extra_models = get("AGENTD_MODELS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .collect();
        let work = WorkConfig::from_lookup(&get)?;
        let streams = StreamsConfig::from_lookup(&get)?;
        let agent_loop = LoopConfig::from_lookup(&get)?;
        let webhooks = hooks::from_lookup(&get)?;
        let scheduler = SchedulerConfig::from_lookup(&get)?;
        let request_timeout =
            std::time::Duration::from_millis(number(&get, "AGENTD_REQUEST_TIMEOUT_MS", 60_000)?);
        Ok(Self {
            addr,
            store,
            blobs,
            secrets,
            keys,
            telemetry,
            extra_models,
            work,
            streams,
            agent_loop,
            webhooks,
            scheduler,
            request_timeout,
        })
    }

    /// Whether `cloud` environments may be served (a runtime key is set) on
    /// the Postgres store, which the cloud egress swap does not read.
    pub fn cloud_swap_misses_secrets(&self) -> bool {
        matches!(self.secrets, SecretsConfig::Postgres(_)) && !self.keys.runtime.is_empty()
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

pub(super) fn flag(
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

/// An endpoint override from `var`, refused when it would send AWS traffic in
/// clear ([`aws_common::endpoint_problem`]).
fn endpoint(get: &impl Fn(&str) -> Option<String>, var: &'static str) -> Result<Option<String>, ConfigError> {
    match get(var) {
        Some(e) => match aws_common::endpoint_problem(&e) {
            Some(problem) => Err(ConfigError { var, reason: format!("`{e}` {problem}") }),
            None => Ok(Some(e)),
        },
        None => Ok(None),
    }
}

mod blobs;
mod hooks;
mod keys;
mod role;
mod scheduler;
mod secrets;
mod store;
mod streams;
mod telemetry;
mod work;

pub use blobs::BlobsConfig;
pub use keys::KeysConfig;
pub use role::{LoopConfig, Role};
pub use scheduler::SchedulerConfig;
pub use secrets::SecretsConfig;
pub use store::StoreConfig;
pub use streams::StreamsConfig;
pub use telemetry::TelemetryConfig;
pub use work::WorkConfig;

#[cfg(test)]
mod tests;
