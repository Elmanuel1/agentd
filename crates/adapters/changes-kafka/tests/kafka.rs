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

/// Produce the change `n` of `table` to `topic`, on `partition` when given.
async fn send(producer: &FutureProducer, topic: &str, table: &str, n: i64, partition: Option<i32>) {
    let value = serde_json::json!({"op": "c", "before": null, "after": {"n": n}, "source": {"table": table}})
        .to_string();
    let mut record = FutureRecord::<str, str>::to(topic).key("k").payload(&value);
    record.partition = partition;
    producer.send(record, Duration::from_secs(10)).await.map_err(|(e, _)| e).unwrap();
}

/// Wait until `done`; fail when it is still false after 30 s.
async fn until(done: impl Fn() -> bool) {
    for _ in 0..120 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    assert!(done(), "not met within 30 s");
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
        send(&producer, &topic, table, n, None).await;
    }
    let seen = Arc::new(Seen { table, fail_once: 2, failed: Mutex::new(false), seen: Mutex::new(vec![]) });
    let tasks = KafkaChanges::new(&brokers, "agentdtest").spawn(vec![seen.clone()]);
    until(|| seen.seen.lock().unwrap().len() >= 4).await;
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
    until(|| !seen.0.lock().unwrap().is_empty()).await;
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
    send(&producer, &topic, table, 1, None).await;
    let poll = Duration::from_secs(10);
    let first = Slow::new(table, poll * 2);
    let feed = KafkaChanges::new(&brokers, "agentdtest").with_max_poll_interval(poll);
    let tasks = feed.spawn(vec![first.clone()]);
    until(|| *first.tries.lock().unwrap() > 0).await;
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
    send(&producer, &topic, table, 1, None).await;
    let poll = Duration::from_secs(10);
    let hangs = |hang| Arc::new(Hangs { table, hang, tries: Mutex::new(0), applied: Mutex::new(0) });
    let first = hangs(poll + Duration::from_secs(5));
    let feed = KafkaChanges::new(&brokers, "agentdtest").with_max_poll_interval(poll);
    let tasks = feed.spawn(vec![first.clone()]);
    until(|| *first.tries.lock().unwrap() > 1).await;
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
    send(&producer, &topic, table, 1, None).await;
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
    until(|| *first.tries.lock().unwrap() > 1).await;
    let second =
        Arc::new(Hangs { table, hang: Duration::ZERO, tries: Mutex::new(0), applied: Mutex::new(0) });
    let joined = feed.spawn(vec![second.clone()]);
    tokio::time::sleep(poll * 2).await;

    assert_eq!(*first.applied.lock().unwrap(), 1, "the long first attempt applied");
    assert_eq!(*second.tries.lock().unwrap(), 0, "the consumer kept the partition");
    tasks.iter().chain(&joined).for_each(tokio::task::JoinHandle::abort);
}

/// Never finishes the change `n = stuck_on`; records the `n` of each other one it applies.
struct Stuck {
    table: &'static str,
    stuck_on: Option<i64>,
    started: Mutex<bool>,
    applied: Mutex<Vec<i64>>,
}

impl NativeChangeHandler for Stuck {
    fn source(&self) -> Source {
        Source::Table(self.table.into())
    }

    fn ordered(&self) -> bool {
        true
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let n = change.after.as_ref().and_then(|r| r["n"].as_i64()).unwrap();
        *self.started.lock().unwrap() = true;
        if Some(n) == self.stuck_on {
            std::future::pending::<()>().await;
        }
        self.applied.lock().unwrap().push(n);
        Ok(())
    }
}

#[tokio::test]
async fn a_record_whose_partition_is_revoked_mid_attempt_is_applied_once_by_its_next_owner() {
    use rdkafka::admin::{AdminClient, AdminOptions, NewTopic, TopicReplication};
    use rdkafka::client::DefaultClientContext;

    let Some(brokers) = brokers() else { return };
    let table: &'static str = Box::leak(format!("t{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let topic = format!("agentdtest.public.{table}");
    let admin: AdminClient<DefaultClientContext> =
        ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    let created = admin
        .create_topics(&[NewTopic::new(&topic, 2, TopicReplication::Fixed(1))], &AdminOptions::new())
        .await
        .unwrap();
    created.into_iter().for_each(|r| _ = r.unwrap());
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    send(&producer, &topic, table, 1, Some(0)).await;
    let stuck = |stuck_on| {
        Arc::new(Stuck { table, stuck_on, started: Mutex::new(false), applied: Mutex::new(vec![]) })
    };
    let first = stuck(Some(1));
    let feed = KafkaChanges::new(&brokers, "agentdtest").with_max_poll_interval(Duration::from_secs(10));
    let tasks = feed.spawn(vec![first.clone()]);
    until(|| *first.started.lock().unwrap()).await;
    let second = stuck(None);
    let joined = feed.spawn(vec![second.clone()]);
    until(|| !second.applied.lock().unwrap().is_empty()).await;
    send(&producer, &topic, table, 2, Some(1)).await;
    until(|| !first.applied.lock().unwrap().is_empty()).await;

    assert_eq!(
        *second.applied.lock().unwrap(),
        vec![1],
        "its next owner applied it, nothing committed before"
    );
    assert_eq!(
        *first.applied.lock().unwrap(),
        vec![2],
        "the revoked attempt ended and its partition moved on"
    );
    tasks.iter().chain(&joined).for_each(tokio::task::JoinHandle::abort);
}

/// What the readers log, kept for the test to read.
#[derive(Clone, Default)]
struct Logged(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logged {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn a_topic_made_after_the_reader_starts_is_read_without_a_reconnect() {
    // A table with no change yet has no topic: the reader waits on the same
    // subscription and reads the first record once Debezium writes it. The
    // test runtime runs the reader's task on this thread, so its log is here.
    let Some(brokers) = brokers() else { return };
    let logged = Logged::default();
    let writer = logged.clone();
    let subscriber = tracing_subscriber::fmt().with_ansi(false).with_writer(move || writer.clone()).finish();
    let _log = tracing::subscriber::set_default(subscriber);
    let table: &'static str = Box::leak(format!("t{}", uuid::Uuid::new_v4().simple()).into_boxed_str());
    let seen = Arc::new(Seen { table, fail_once: -1, failed: Mutex::new(false), seen: Mutex::new(vec![]) });
    let tasks = KafkaChanges::new(&brokers, "agentdtest").spawn(vec![seen.clone()]);
    let read = || String::from_utf8(logged.0.lock().unwrap().clone()).unwrap();
    until(|| read().contains("the topic does not exist yet")).await;
    let producer: FutureProducer = ClientConfig::new().set("bootstrap.servers", &brokers).create().unwrap();
    send(&producer, &format!("agentdtest.public.{table}"), table, 1, None).await;
    until(|| !seen.seen.lock().unwrap().is_empty()).await;
    assert_eq!(*seen.seen.lock().unwrap(), vec![1]);
    tasks.iter().for_each(tokio::task::JoinHandle::abort);
    let log = String::from_utf8(logged.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("the topic does not exist yet"), "the wait is logged: {log}");
    assert!(!log.contains("reconnecting"), "the reader kept its consumer: {log}");
}
