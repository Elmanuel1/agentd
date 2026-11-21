//! Session streams across replicas, and how long archived sessions keep
//! their events.

use std::time::Duration;

use super::kafka::KafkaConfig;
use super::{ConfigError, flag, number};

/// How replicas share session changes, and event retention.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamsConfig {
    /// `AGENTD_VALKEY_URL` (required): the Valkey every replica signals
    /// through, and whose streams carry the change feed.
    pub valkey_url: String,
    /// `AGENTD_VALKEY_CLUSTER` (`false`): the URL is a cluster's
    /// configuration endpoint (cluster mode enabled).
    pub valkey_cluster: bool,
    /// `AGENTD_VALKEY_CA_CERT`: the PEM file a `rediss://` server's
    /// certificate must chain to. Unset, the operating system's roots.
    pub valkey_ca: Option<String>,
    /// `AGENTD_KAFKA_BROKERS` (`host:port,…`): read the change feed from
    /// Kafka's topics. Unset, it is read from Valkey Streams.
    pub kafka_brokers: Option<String>,
    /// How the feed reaches Kafka: TLS, SASL or IAM.
    pub kafka: KafkaConfig,
    /// `AGENTD_CHANGES_PREFIX` (`agentd`): Debezium's `topic.prefix`, which
    /// names the change streams `<prefix>.public.<table>`.
    pub changes_prefix: String,
    /// `AGENTD_CHANGES_SLOT` (`agentd_changes`): Debezium's replication slot, whose WAL agentd reports.
    pub changes_slot: String,
    /// `AGENTD_EVENT_RETENTION_DAYS`: archived sessions lose their events
    /// this long after archiving. Unset, events are kept.
    pub retention: Option<Duration>,
}

/// Shows whether a Valkey URL is set, never the URL, which may hold a password.
impl std::fmt::Debug for StreamsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamsConfig")
            .field("valkey_url", &"***")
            .field("valkey_cluster", &self.valkey_cluster)
            .field("valkey_ca", &self.valkey_ca)
            .field("kafka_brokers", &self.kafka_brokers)
            .field("kafka", &self.kafka)
            .field("changes_prefix", &self.changes_prefix)
            .field("changes_slot", &self.changes_slot)
            .field("retention", &self.retention)
            .finish()
    }
}

const DAY: u64 = 24 * 60 * 60;

/// A CA only means something to a `rediss://` URL.
fn valkey_ca(get: &impl Fn(&str) -> Option<String>) -> Result<Option<String>, ConfigError> {
    let ca = get("AGENTD_VALKEY_CA_CERT").filter(|c| !c.is_empty());
    let tls = get("AGENTD_VALKEY_URL").is_some_and(|u| u.starts_with("rediss://"));
    match ca.is_some() && !tls {
        true => Err(ConfigError {
            var: "AGENTD_VALKEY_CA_CERT",
            reason: "needs a rediss:// AGENTD_VALKEY_URL".into(),
        }),
        false => Ok(ca),
    }
}

impl StreamsConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let days: Option<u64> = get("AGENTD_EVENT_RETENTION_DAYS")
            .map(|_| number(get, "AGENTD_EVENT_RETENTION_DAYS", 1))
            .transpose()?;
        Ok(Self {
            valkey_url: get("AGENTD_VALKEY_URL").filter(|u| !u.is_empty()).ok_or_else(|| ConfigError {
                var: "AGENTD_VALKEY_URL",
                reason: "required: the replicas' signals and the change feed".into(),
            })?,
            valkey_cluster: flag(get, "AGENTD_VALKEY_CLUSTER", false)?,
            valkey_ca: valkey_ca(get)?,
            kafka_brokers: get("AGENTD_KAFKA_BROKERS").filter(|b| !b.is_empty()),
            kafka: KafkaConfig::from_lookup(get)?,
            changes_prefix: get("AGENTD_CHANGES_PREFIX").unwrap_or_else(|| "agentd".into()),
            changes_slot: get("AGENTD_CHANGES_SLOT").unwrap_or_else(|| "agentd_changes".into()),
            retention: days.map(|d| Duration::from_secs(d.saturating_mul(DAY))),
        })
    }
}
