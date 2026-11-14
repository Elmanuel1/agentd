//! The change feed agentd reads: Kafka when `AGENTD_KAFKA_BROKERS` is set,
//! else Valkey Streams.

use std::sync::Arc;

use changes_kafka::KafkaChanges;
use shared::ports::changes::ChangeHandler;
use shared::ports::fire::FireRegistry;
use signals_valkey::{Valkey, ValkeyChanges};
use tokio::task::JoinHandle;

use crate::config::StreamsConfig;

/// Where the table changes and an external scheduler's fires arrive.
#[derive(Debug, Clone)]
pub enum ChangeFeed {
    /// Valkey Streams, read by this replica as its own consumer.
    Valkey(ValkeyChanges),
    /// Kafka topics.
    Kafka(KafkaChanges),
}

impl ChangeFeed {
    /// The feed `streams` selects; Valkey Streams on `valkey` without Kafka.
    pub(crate) fn from_config(streams: &StreamsConfig, valkey: &Valkey) -> Self {
        let prefix = &streams.changes_prefix;
        match &streams.kafka_brokers {
            Some(brokers) => Self::Kafka(KafkaChanges::new(brokers, prefix)),
            None => {
                let consumer = format!("agentd-{}", uuid::Uuid::new_v4());
                Self::Valkey(ValkeyChanges::new(valkey, prefix, &consumer))
            }
        }
    }

    /// Read each handler's table on a task of its own.
    pub(crate) fn spawn(&self, handlers: Vec<Arc<dyn ChangeHandler>>) -> Vec<JoinHandle<()>> {
        match self {
            Self::Valkey(feed) => feed.spawn(handlers),
            Self::Kafka(feed) => feed.spawn(handlers),
        }
    }

    /// Run an external scheduler's fires.
    pub(crate) fn spawn_fires(&self, fires: Arc<FireRegistry>) -> JoinHandle<()> {
        match self {
            Self::Valkey(feed) => feed.spawn_fires(fires),
            Self::Kafka(feed) => feed.spawn_fires(fires),
        }
    }
}
