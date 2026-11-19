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

/// Fails every record until `failing_for` has passed since its first try;
/// counts its tries and the records it applied.
struct Slow {
    table: &'static str,
    failing_for: Duration,
    first: Mutex<Option<std::time::Instant>>,
    tries: Mutex<usize>,
    applied: Mutex<usize>,
}

impl Slow {
    fn new(table: &'static str, failing_for: Duration) -> Arc<Self> {
        Arc::new(Self {
            table,
            failing_for,
            first: Mutex::new(None),
            tries: Mutex::new(0),
            applied: Mutex::new(0),
        })
    }
}

impl NativeChangeHandler for Slow {
    fn source(&self) -> Source {
        Source::Table(self.table.into())
    }

    fn ordered(&self) -> bool {
        true
    }

    async fn handle(&self, _: &Change) -> Result<(), BackendError> {
        *self.tries.lock().unwrap() += 1;
        let first = *self.first.lock().unwrap().get_or_insert_with(std::time::Instant::now);
        if first.elapsed() < self.failing_for {
            return Err(BackendError::new(std::io::Error::other("still down")));
        }
        *self.applied.lock().unwrap() += 1;
        Ok(())
    }
}

#[tokio::test]
async fn a_record_retried_past_max_poll_interval_keeps_one_reader() {
    let Some(brokers) = brokers() else { return };
    let table: &'static str = Box::leak(format!("t{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    let topic = format!("agentdtest.public.{table}");
    let value = serde_json::json!({"op": "c", "before": null, "after": {"n": 1}, "source": {"table": table}})
        .to_string();
    let record = FutureRecord::<str, str>::to(&topic).key("k").payload(&value);
    producer.send(record, Duration::from_secs(10)).await.map_err(|(e, _)| e).unwrap();
    let poll = Duration::from_secs(10);
    let first = Slow::new(table, poll * 2);
    let feed = KafkaChanges::new(&brokers, "agentdtest").with_max_poll_interval(poll);
    let tasks = feed.spawn(vec![first.clone()]);
    for _ in 0..120 {
        if *first.tries.lock().unwrap() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let second = Slow::new(table, Duration::ZERO);
    let joined = feed.spawn(vec![second.clone()]);
    tokio::time::sleep(poll * 3).await;

    let applied = *first.applied.lock().unwrap() + *second.applied.lock().unwrap();
    let tries = (*first.tries.lock().unwrap(), *second.tries.lock().unwrap());
    assert_eq!(applied, 1, "the record applies once: {tries:?}");
    assert_eq!(*second.tries.lock().unwrap(), 0, "the retrying consumer kept the partition");
    assert!(*first.tries.lock().unwrap() > 2, "retried past max.poll.interval.ms");
    tasks.iter().chain(&joined).for_each(tokio::task::JoinHandle::abort);
}

/// Fails its first try at once, then takes `hang` before it applies.
struct Hangs {
    table: &'static str,
    hang: Duration,
    tries: Mutex<usize>,
    applied: Mutex<usize>,
}

impl NativeChangeHandler for Hangs {
    fn source(&self) -> Source {
        Source::Table(self.table.into())
    }

    fn ordered(&self) -> bool {
        true
    }

    async fn handle(&self, _: &Change) -> Result<(), BackendError> {
        let first = {
            let mut tries = self.tries.lock().unwrap();
            *tries += 1;
            *tries == 1
        };
        if first {
            return Err(BackendError::new(std::io::Error::other("down")));
        }
        tokio::time::sleep(self.hang).await;
        *self.applied.lock().unwrap() += 1;
        Ok(())
    }
}

#[tokio::test]
async fn a_retry_attempt_longer_than_max_poll_interval_keeps_one_reader() {
    let Some(brokers) = brokers() else { return };
    let table: &'static str = Box::leak(format!("t{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    let topic = format!("agentdtest.public.{table}");
    let value = serde_json::json!({"op": "c", "before": null, "after": {"n": 1}, "source": {"table": table}})
        .to_string();
    let record = FutureRecord::<str, str>::to(&topic).key("k").payload(&value);
    producer.send(record, Duration::from_secs(10)).await.map_err(|(e, _)| e).unwrap();
    let poll = Duration::from_secs(10);
    let hangs = |hang| Arc::new(Hangs { table, hang, tries: Mutex::new(0), applied: Mutex::new(0) });
    let first = hangs(poll + Duration::from_secs(5));
    let feed = KafkaChanges::new(&brokers, "agentdtest").with_max_poll_interval(poll);
    let tasks = feed.spawn(vec![first.clone()]);
    for _ in 0..120 {
        if *first.tries.lock().unwrap() > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let second = hangs(Duration::ZERO);
    let joined = feed.spawn(vec![second.clone()]);
    tokio::time::sleep(poll * 2).await;

    assert_eq!(*first.applied.lock().unwrap(), 1, "the long attempt applied");
    assert_eq!(*second.tries.lock().unwrap(), 0, "the retrying consumer kept the partition");
    tasks.iter().chain(&joined).for_each(tokio::task::JoinHandle::abort);
}

#[tokio::test]
async fn a_first_attempt_longer_than_max_poll_interval_keeps_one_reader() {
    let Some(brokers) = brokers() else { return };
    let table: &'static str = Box::leak(format!("t{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    let topic = format!("agentdtest.public.{table}");
    let value = serde_json::json!({"op": "c", "before": null, "after": {"n": 1}, "source": {"table": table}})
        .to_string();
    let record = FutureRecord::<str, str>::to(&topic).key("k").payload(&value);
    producer.send(record, Duration::from_secs(10)).await.map_err(|(e, _)| e).unwrap();
    let poll = Duration::from_secs(10);
    // Its first try is the long one: it applies after 15 s.
    let first = Arc::new(Hangs {
        table,
        hang: poll + Duration::from_secs(5),
        tries: Mutex::new(1),
        applied: Mutex::new(0),
    });
    let feed = KafkaChanges::new(&brokers, "agentdtest").with_max_poll_interval(poll);
    let tasks = feed.spawn(vec![first.clone()]);
    for _ in 0..120 {
        if *first.tries.lock().unwrap() > 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let second =
        Arc::new(Hangs { table, hang: Duration::ZERO, tries: Mutex::new(0), applied: Mutex::new(0) });
    let joined = feed.spawn(vec![second.clone()]);
    tokio::time::sleep(poll * 2).await;

    assert_eq!(*first.applied.lock().unwrap(), 1, "the long first attempt applied");
    assert_eq!(*second.tries.lock().unwrap(), 0, "the consumer kept the partition");
    tasks.iter().chain(&joined).for_each(tokio::task::JoinHandle::abort);
}
