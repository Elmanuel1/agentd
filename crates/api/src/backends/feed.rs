//! The change feed agentd reads: Kafka when `AGENTD_KAFKA_BROKERS` is set,
//! else Valkey Streams.

use std::sync::Arc;

use aws_common::MskAuthToken;
use changes_kafka::{KafkaChanges, KafkaClient};
use shared::BackendError;
use shared::ports::BoxFuture;
use shared::ports::changes::{Change, ChangeHandler, Source};
use shared::ports::fire::FireRegistry;
use signals_valkey::{Valkey, ValkeyChanges};
use tokio::task::JoinHandle;

use crate::config::{KafkaAuth, StreamsConfig};

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
    pub(crate) async fn from_config(streams: &StreamsConfig, valkey: &Valkey) -> Self {
        let prefix = &streams.changes_prefix;
        match &streams.kafka_brokers {
            Some(brokers) => {
                let client = KafkaClient::new(streams.kafka.properties.clone());
                let client = match streams.kafka.auth {
                    KafkaAuth::Password => client,
                    KafkaAuth::AwsIam => client
                        .with_bearer(Arc::new(MskAuthToken::new(aws_common::sdk_config(None, None).await))),
                };
                Self::Kafka(KafkaChanges::new(brokers, prefix).with_client(client))
            }
            None => {
                let consumer = format!("agentd-{}", uuid::Uuid::new_v4());
                Self::Valkey(ValkeyChanges::new(valkey, prefix, &consumer))
            }
        }
    }

    /// Read each table on a task of its own. A table's handlers share its one consumer group, so they
    /// run together on one reader, in the order given; apart, each would get only some of its changes.
    pub(crate) fn spawn(&self, handlers: Vec<Arc<dyn ChangeHandler>>) -> Vec<JoinHandle<()>> {
        let handlers = together(handlers);
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

/// One handler per table, running that table's handlers in the order given.
fn together(handlers: Vec<Arc<dyn ChangeHandler>>) -> Vec<Arc<dyn ChangeHandler>> {
    let mut by_table: Vec<Together> = Vec::new();
    for h in handlers {
        let source = h.source();
        match by_table.iter_mut().find(|t| t.source == source) {
            Some(table) => table.all.push(h),
            None => by_table.push(Together { source, all: vec![h] }),
        }
    }
    by_table.into_iter().map(|t| Arc::new(t) as Arc<dyn ChangeHandler>).collect()
}

/// The handlers of one table, each run on every change in turn; an error leaves the change to be handled
/// again by all of them, so each must take a repeat.
struct Together {
    source: Source,
    all: Vec<Arc<dyn ChangeHandler>>,
}

impl ChangeHandler for Together {
    fn source(&self) -> Source {
        self.source.clone()
    }

    fn ordered(&self) -> bool {
        self.all.iter().any(|h| h.ordered())
    }

    fn handle<'a>(&'a self, change: &'a Change) -> BoxFuture<'a, Result<(), BackendError>> {
        Box::pin(async move {
            for h in &self.all {
                h.handle(change).await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use shared::ports::changes::{ChangeOp, NativeChangeHandler};

    use super::*;

    /// Records each change it handles under `name`.
    struct Seen(Source, &'static str, Arc<Mutex<Vec<&'static str>>>);

    impl NativeChangeHandler for Seen {
        fn source(&self) -> Source {
            self.0.clone()
        }
        async fn handle(&self, _: &Change) -> Result<(), BackendError> {
            self.2.lock().unwrap().push(self.1);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_table_s_handlers_share_one_reader_and_each_gets_every_change() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let on = |table: &str, name| -> Arc<dyn ChangeHandler> {
            Arc::new(Seen(Source::Table(table.into()), name, seen.clone()))
        };
        let readers =
            together(vec![on("api_keys", "history"), on("sessions", "other"), on("api_keys", "revokes")]);
        assert_eq!(readers.len(), 2, "one reader per table");
        let change = Change {
            table: "api_keys".into(),
            op: ChangeOp::Update,
            position: None,
            before: None,
            after: None,
        };
        readers[0].handle(&change).await.unwrap();
        assert_eq!(*seen.lock().unwrap(), ["history", "revokes"]);
    }
}
