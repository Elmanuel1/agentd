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
    /// `AGENTD_TEMPORAL_TASK_QUEUE` (`agentd`).
    Temporal(TemporalConfig),
}

impl SchedulerConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        match selector(get, "AGENTD_SCHEDULER", "postgres").as_str() {
            "postgres" => Ok(Self::Postgres),
            "temporal" => Ok(Self::Temporal(TemporalConfig {
                address: selector(get, "AGENTD_TEMPORAL_ADDRESS", "http://localhost:7233"),
                namespace: selector(get, "AGENTD_TEMPORAL_NAMESPACE", "default"),
                task_queue: selector(get, "AGENTD_TEMPORAL_TASK_QUEUE", "agentd"),
            })),
            other => Err(unknown("AGENTD_SCHEDULER", other)),
        }
    }
}
