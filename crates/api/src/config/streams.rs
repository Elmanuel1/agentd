//! Session streams across replicas, and how long archived sessions keep
//! their events.

use std::time::Duration;

use super::{number, ConfigError};

/// How replicas share session changes, and event retention.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct StreamsConfig {
    /// `AGENTD_VALKEY_URL`: the Valkey every replica signals through. Unset,
    /// signals stay in this process, which suits a single replica.
    pub valkey_url: Option<String>,
    /// `AGENTD_EVENT_RETENTION_DAYS`: archived sessions lose their events
    /// this long after archiving. Unset, events are kept.
    pub retention: Option<Duration>,
}

/// Shows whether a Valkey URL is set, never the URL, which may hold a password.
impl std::fmt::Debug for StreamsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamsConfig")
            .field("valkey_url", &self.valkey_url.as_ref().map(|_| "***"))
            .field("retention", &self.retention)
            .finish()
    }
}

const DAY: u64 = 24 * 60 * 60;

impl StreamsConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let days: Option<u64> = get("AGENTD_EVENT_RETENTION_DAYS")
            .map(|_| number(get, "AGENTD_EVENT_RETENTION_DAYS", 1))
            .transpose()?;
        Ok(Self {
            valkey_url: get("AGENTD_VALKEY_URL").filter(|u| !u.is_empty()),
            retention: days.map(|d| Duration::from_secs(d.saturating_mul(DAY))),
        })
    }
}
