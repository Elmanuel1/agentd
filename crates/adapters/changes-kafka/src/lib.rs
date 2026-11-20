//! changes-kafka: the change feed over Kafka. Debezium Server writes each
//! table's committed changes to the topic `<prefix>.public.<table>`, and the
//! logical messages to `<prefix>.message` (its Kafka sink, JSON values without
//! schemas), an external scheduler writes its
//! fires to `<prefix>.fires`, and the replicas read each topic in the
//! consumer group `agentd.<topic>`. A record's offset is committed once its
//! handler succeeds; each record is handled, and a failed one retried in
//! place, while the consumer keeps polling ([`retry`]), so nothing behind it
//! in its partition overtakes it. An ordered handler's
//! topic has one partition, which the group gives to one replica at a time.
//! The topics' retention trims them.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rdkafka::Message;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer};
use rdkafka::error::KafkaError;
use shared::ports::ScheduleSpec;
use shared::ports::changes::{self, ChangeHandler};
use shared::ports::fire::{self, FireRegistry};

mod client;
mod retry;

pub use client::KafkaClient;
use client::KafkaConsumer;

/// The pause before a failed record, or a failed read, is tried again.
const BACKOFF: Duration = Duration::from_secs(1);
/// librdkafka's `max.poll.interval.ms` default: a consumer that polls less often leaves its group.
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(300);
/// librdkafka's `session.timeout.ms` default, which may not pass `max.poll.interval.ms`.
const SESSION_TIMEOUT: Duration = Duration::from_secs(45);

/// Reads the change topics of one Kafka cluster.
#[derive(Debug, Clone)]
pub struct KafkaChanges {
    brokers: String,
    prefix: String,
    max_poll_interval: Duration,
    client: KafkaClient,
}

/// What reads one topic.
#[derive(Clone)]
pub(crate) enum Reader {
    Changes(Arc<dyn ChangeHandler>),
    Fires(Arc<FireRegistry>),
}

/// A fire record's value.
#[derive(serde::Deserialize)]
struct FireRecord {
    spec: ScheduleSpec,
    scheduled_at: DateTime<Utc>,
}

impl Reader {
    pub(crate) async fn consume(&self, value: Option<&str>, id: &str) -> bool {
        match self {
            Self::Changes(handler) => changes::deliver(handler.as_ref(), value, id).await,
            Self::Fires(registry) => {
                let record = value.and_then(|v| serde_json::from_str::<FireRecord>(v).ok());
                fire::deliver(registry, record.map(|r| (r.spec, r.scheduled_at)), id).await
            }
        }
    }
}

impl KafkaChanges {
    /// The topics of Debezium's `prefix` (its `topic.prefix`) on the Kafka at
    /// `brokers` (`host:port,…`).
    pub fn new(brokers: &str, prefix: &str) -> Self {
        Self {
            brokers: brokers.to_owned(),
            prefix: prefix.to_owned(),
            max_poll_interval: MAX_POLL_INTERVAL,
            client: KafkaClient::default(),
        }
    }

    /// Reaching the cluster as `client` says (TLS, SASL).
    #[must_use]
    pub fn with_client(mut self, client: KafkaClient) -> Self {
        self.client = client;
        self
    }

    /// With librdkafka's `max.poll.interval.ms` at `interval` instead of 300 s.
    #[must_use]
    pub fn with_max_poll_interval(mut self, interval: Duration) -> Self {
        self.max_poll_interval = interval;
        self
    }

    /// Read each handler's topic on a task of its own.
    pub fn spawn(&self, handlers: Vec<Arc<dyn ChangeHandler>>) -> Vec<tokio::task::JoinHandle<()>> {
        handlers
            .into_iter()
            .map(|h| {
                let topic = h.source().topic(&self.prefix);
                tokio::spawn(self.clone().run(topic, Reader::Changes(h)))
            })
            .collect()
    }

    /// Run the fires an external scheduler writes to `<prefix>.fires`, each
    /// record's value `{"spec": …, "scheduled_at": "<RFC 3339>"}`.
    pub fn spawn_fires(&self, fires: Arc<FireRegistry>) -> tokio::task::JoinHandle<()> {
        let topic = format!("{}.fires", self.prefix);
        tokio::spawn(self.clone().run(topic, Reader::Fires(fires)))
    }

    async fn run(self, topic: String, reader: Reader) {
        loop {
            if let Err(e) = self.client.ready().await {
                tracing::warn!(%topic, error = %e, "changes: no OAUTHBEARER token; trying again");
            } else if let Err(e) = self.serve(&topic, &reader).await {
                tracing::warn!(%topic, error = %e, "changes: the topic read failed; reconnecting");
            }
            tokio::time::sleep(BACKOFF).await;
        }
    }

    async fn serve(&self, topic: &str, reader: &Reader) -> Result<(), KafkaError> {
        let mut config = ClientConfig::new();
        self.client.apply(&mut config);
        let consumer: KafkaConsumer = config
            .set("bootstrap.servers", &self.brokers)
            .set("group.id", format!("agentd.{topic}"))
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .set("max.poll.interval.ms", self.max_poll_interval.as_millis().to_string())
            .set("session.timeout.ms", self.max_poll_interval.min(SESSION_TIMEOUT).as_millis().to_string())
            // A join moves only the partitions balance needs (a one-partition topic stays with its holder);
            // a moved partition's attempt ends uncommitted, for its next owner to read.
            .set("partition.assignment.strategy", "cooperative-sticky")
            .create_with_context(self.client.context())?;
        consumer.subscribe(&[topic])?;
        loop {
            let record = consumer.recv().await?;
            let id = format!("{}/{}@{}", record.topic(), record.partition(), record.offset());
            if retry::until_taken(&consumer, &record, reader, &id).await? {
                consumer.commit_message(&record, CommitMode::Async)?;
            }
        }
    }
}
