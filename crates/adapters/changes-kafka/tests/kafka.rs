//! Against the Kafka at `AGENTD_KAFKA_BROKERS`; skipped without it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use changes_kafka::KafkaChanges;
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use shared::BackendError;
use shared::ports::changes::{Change, NativeChangeHandler, Source};

fn brokers() -> Option<String> {
    std::env::var("AGENTD_KAFKA_BROKERS").ok()
}

/// Records the `n` of each change it is handed, failing the first time it sees `fail_once`.
struct Seen {
    table: &'static str,
    fail_once: i64,
    failed: Mutex<bool>,
    seen: Mutex<Vec<i64>>,
}

impl NativeChangeHandler for Seen {
    fn source(&self) -> Source {
        Source::Table(self.table.into())
    }

    fn ordered(&self) -> bool {
        true
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let n = change.after.as_ref().and_then(|r| r["n"].as_i64()).unwrap();
        self.seen.lock().unwrap().push(n);
        let mut failed = self.failed.lock().unwrap();
        if n == self.fail_once && !*failed {
            *failed = true;
            return Err(BackendError::new(std::io::Error::other("first try fails")));
        }
        Ok(())
    }
}

#[tokio::test]
async fn a_failed_record_is_retried_before_the_ones_behind_it() {
    let Some(brokers) = brokers() else { return };
    let table: &'static str = Box::leak(format!("t{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    let topic = format!("agentdtest.public.{table}");
    for n in 1..=3 {
        let value =
            serde_json::json!({"op": "c", "before": null, "after": {"n": n}, "source": {"table": table}})
                .to_string();
        let record = FutureRecord::<str, str>::to(&topic).key("k").payload(&value);
        producer.send(record, Duration::from_secs(10)).await.map_err(|(e, _)| e).unwrap();
    }
    let seen = Arc::new(Seen { table, fail_once: 2, failed: Mutex::new(false), seen: Mutex::new(vec![]) });
    let tasks = KafkaChanges::new(&brokers, "agentdtest").spawn(vec![seen.clone()]);
    for _ in 0..120 {
        if seen.seen.lock().unwrap().len() >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(*seen.seen.lock().unwrap(), vec![1, 2, 2, 3]);
    tasks.iter().for_each(tokio::task::JoinHandle::abort);
}

/// Records the content of each message it is handed.
struct Messages(Mutex<Vec<serde_json::Value>>);

impl NativeChangeHandler for Messages {
    fn source(&self) -> Source {
        Source::Messages
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        self.0.lock().unwrap().push(change.after.clone().unwrap());
        Ok(())
    }
}

#[tokio::test]
async fn a_message_handler_reads_the_message_topic() {
    let Some(brokers) = brokers() else { return };
    let prefix = format!("agentdtest{}", uuid::Uuid::new_v4().simple());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    // `{"table":"vaults"}`, base64 as Debezium sends a message's bytes.
    let content = "eyJ0YWJsZSI6InZhdWx0cyJ9";
    let value = serde_json::json!({
        "op": "m",
        "source": {"lsn": 7, "txId": 9, "ts_ms": 1},
        "message": {"prefix": "agentd-history", "content": content},
    })
    .to_string();
    let topic = Source::Messages.topic(&prefix);
    let record = FutureRecord::<str, str>::to(&topic).key("k").payload(&value);
    producer.send(record, Duration::from_secs(10)).await.map_err(|(e, _)| e).unwrap();
    let seen = Arc::new(Messages(Mutex::new(vec![])));
    let tasks = KafkaChanges::new(&brokers, &prefix).spawn(vec![seen.clone()]);
    for _ in 0..120 {
        if !seen.0.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert_eq!(*seen.0.lock().unwrap(), vec![serde_json::json!({"table": "vaults"})]);
    tasks.iter().for_each(tokio::task::JoinHandle::abort);
}
