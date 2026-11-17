//! changes-kafka: the change feed over Kafka. Debezium Server writes each
//! table's committed changes to the topic `<prefix>.public.<table>`, and the
//! logical messages to `<prefix>.message` (its Kafka sink, JSON values without
//! schemas), an external scheduler writes its
//! fires to `<prefix>.fires`, and the replicas read each topic in the
//! consumer group `agentd.<topic>`. A record's offset is committed once its
//! handler succeeds; a failed record is retried in place, so nothing behind
//! it in its partition overtakes it. An ordered handler's topic has one
//! partition, which the group gives to one replica at a time. The topics'
//! retention trims them.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rdkafka::Message;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::error::KafkaError;
use shared::ports::ScheduleSpec;
use shared::ports::changes::{self, ChangeHandler};
use shared::ports::fire::{self, FireRegistry};

/// The pause before a failed record, or a failed read, is tried again.
const BACKOFF: Duration = Duration::from_secs(1);

/// Reads the change topics of one Kafka cluster.
#[derive(Debug, Clone)]
pub struct KafkaChanges {
    brokers: String,
    prefix: String,
}

/// What reads one topic.
#[derive(Clone)]
enum Reader {
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
    async fn consume(&self, value: Option<&str>, id: &str) -> bool {
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
        Self { brokers: brokers.to_owned(), prefix: prefix.to_owned() }
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
            if let Err(e) = self.serve(&topic, &reader).await {
                tracing::warn!(%topic, error = %e, "changes: the topic read failed; reconnecting");
            }
            tokio::time::sleep(BACKOFF).await;
        }
    }

    async fn serve(&self, topic: &str, reader: &Reader) -> Result<(), KafkaError> {
        let consumer: StreamConsumer = ClientConfig::new()
            .set("bootstrap.servers", &self.brokers)
            .set("group.id", format!("agentd.{topic}"))
            .set("enable.auto.commit", "false")
            .set("auto.offset.reset", "earliest")
            .create()?;
        consumer.subscribe(&[topic])?;
        loop {
            let record = consumer.recv().await?;
            let value = record.payload_view::<str>().and_then(Result::ok);
            let id = format!("{}/{}@{}", record.topic(), record.partition(), record.offset());
            while !reader.consume(value, &id).await {
                tokio::time::sleep(BACKOFF).await;
            }
            consumer.commit_message(&record, CommitMode::Async)?;
        }
    }
}
