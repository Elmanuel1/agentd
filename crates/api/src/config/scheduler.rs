//! `AGENTD_SCHEDULER`: the scheduler sink schedule changes go to.

use scheduler_temporal::TemporalConfig;

use super::{ConfigError, selector, unknown};

/// `AGENTD_SCHEDULER`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerConfig {
    /// `postgres` (the default): the built-in scheduler, whose tick every replica runs.
    Postgres,
    /// `temporal`: Temporal Schedules at `AGENTD_TEMPORAL_ADDRESS` (`http://localhost:7233`), in
    /// `AGENTD_TEMPORAL_NAMESPACE` (`default`), whose actions agentd's worker takes from
    /// `AGENTD_TEMPORAL_TASK_QUEUE` (`agentd`). An `https://` address takes
    /// `AGENTD_TEMPORAL_API_KEY` and `AGENTD_TEMPORAL_TLS_SERVER_CA_CERT_PATH`.
    Temporal(TemporalConfig),
}

impl SchedulerConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        match selector(get, "AGENTD_SCHEDULER", "postgres").as_str() {
            "postgres" => Ok(Self::Postgres),
            "temporal" => {
                let address = selector(get, "AGENTD_TEMPORAL_ADDRESS", "http://localhost:7233");
                let tls = address.starts_with("https://");
                Ok(Self::Temporal(TemporalConfig {
                    namespace: selector(get, "AGENTD_TEMPORAL_NAMESPACE", "default"),
                    task_queue: selector(get, "AGENTD_TEMPORAL_TASK_QUEUE", "agentd"),
                    api_key: over_tls(get, "AGENTD_TEMPORAL_API_KEY", tls)?,
                    server_ca: over_tls(get, "AGENTD_TEMPORAL_TLS_SERVER_CA_CERT_PATH", tls)?,
                    address,
                }))
            }
            other => Err(unknown("AGENTD_SCHEDULER", other)),
        }
    }
}

/// A setting only TLS may carry: the API key would travel in clear, and a CA
/// means nothing to plain HTTP.
fn over_tls(
    get: &impl Fn(&str) -> Option<String>,
    var: &'static str,
    tls: bool,
) -> Result<Option<String>, ConfigError> {
    match get(var).filter(|v| !v.is_empty()) {
        Some(_) if !tls => {
            Err(ConfigError { var, reason: "needs an https:// AGENTD_TEMPORAL_ADDRESS".into() })
        }
        value => Ok(value),
    }
}
