//! The feed over TLS and SASL against `docker/kafka/start-secure.sh`'s broker,
//! whose certificate the CA at `AGENTD_KAFKA_TLS_CA` signs; skipped without it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use changes_kafka::{KafkaChanges, KafkaClient};
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, FutureRecord};
use shared::BackendError;
use shared::ports::BoxFuture;
use shared::ports::changes::{Change, NativeChangeHandler, Source};
use shared::ports::credentials::{BearerToken, BearerTokenSource};

const SASL_SSL: &str = "localhost:19093";
const SSL: &str = "localhost:19094";

fn ca() -> Option<String> {
    std::env::var("AGENTD_KAFKA_TLS_CA").ok()
}

fn properties(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

fn plain(ca: &str) -> Vec<(String, String)> {
    properties(&[
        ("security.protocol", "SASL_SSL"),
        ("sasl.mechanism", "PLAIN"),
        ("sasl.username", "agentd"),
        ("sasl.password", "agentd-secret"),
        ("ssl.ca.location", ca),
    ])
}

/// Records the `n` of each change it is handed.
struct Seen {
    table: String,
    seen: Mutex<Vec<i64>>,
}

impl NativeChangeHandler for Seen {
    fn source(&self) -> Source {
        Source::Table(self.table.clone())
    }

    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        self.seen.lock().unwrap().push(change.after.as_ref().and_then(|r| r["n"].as_i64()).unwrap());
        Ok(())
    }
}

/// Write one change over SASL_SSL with PLAIN, then read it through the feed
/// at `brokers` as `client` says.
async fn reads_through(brokers: &str, client: KafkaClient) {
    let ca = ca().unwrap();
    let mut producing = ClientConfig::new();
    producing.set("bootstrap.servers", SASL_SSL);
    for (k, v) in plain(&ca) {
        producing.set(k, v);
    }
    let producer: FutureProducer = producing.create().unwrap();
    let table = format!("t{}", uuid::Uuid::new_v4().simple());
    let value = serde_json::json!({"op": "c", "before": null, "after": {"n": 7}, "source": {"table": table}});
    let (topic, value) = (format!("agentdtest.public.{table}"), value.to_string());
    let record = FutureRecord::<str, str>::to(&topic).key("k").payload(&value);
    producer.send(record, Duration::from_secs(30)).await.map_err(|(e, _)| e).unwrap();

    let seen = Arc::new(Seen { table, seen: Mutex::new(vec![]) });
    let tasks = KafkaChanges::new(brokers, "agentdtest").with_client(client).spawn(vec![seen.clone()]);
    for _ in 0..120 {
        if !seen.seen.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    tasks.iter().for_each(tokio::task::JoinHandle::abort);
    assert_eq!(*seen.seen.lock().unwrap(), vec![7]);
}

#[tokio::test]
async fn sasl_ssl_with_plain_reads_the_feed() {
    let Some(ca) = ca() else { return };
    reads_through(SASL_SSL, KafkaClient::new(plain(&ca))).await;
}

#[tokio::test]
async fn sasl_ssl_with_scram_reads_the_feed() {
    let Some(ca) = ca() else { return };
    let mut scram = plain(&ca);
    scram.retain(|(k, _)| k != "sasl.mechanism");
    scram.push(("sasl.mechanism".into(), "SCRAM-SHA-512".into()));
    reads_through(SASL_SSL, KafkaClient::new(scram)).await;
}

#[tokio::test]
async fn ssl_reads_the_feed() {
    let Some(ca) = ca() else { return };
    let ssl = properties(&[("security.protocol", "SSL"), ("ssl.ca.location", &ca)]);
    reads_through(SSL, KafkaClient::new(ssl)).await;
}

/// Kafka's unsecured OAUTHBEARER tokens (`alg: none`), which the test broker
/// accepts, each after a wait on the runtime's timer, as a credential fetch
/// waits on its IO; counts how many it hands out.
#[derive(Debug, Default)]
struct Unsecured(Mutex<usize>);

impl BearerTokenSource for Unsecured {
    fn token(&self) -> BoxFuture<'_, Result<BearerToken, BackendError>> {
        *self.0.lock().unwrap() += 1;
        Box::pin(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
            let claims = serde_json::json!({"sub": "agentd", "iat": now, "exp": now + 900}).to_string();
            Ok(BearerToken {
                value: format!("{header}.{}.", URL_SAFE_NO_PAD.encode(claims)),
                principal: "agentd".into(),
                expires_at_ms: i64::try_from((now + 900) * 1000).unwrap(),
            })
        })
    }
}

/// On a multi-threaded runtime, as agentd runs.
#[tokio::test(flavor = "multi_thread")]
async fn oauthbearer_reads_the_feed_with_the_sources_tokens() {
    let Some(ca) = ca() else { return };
    let source = Arc::new(Unsecured::default());
    let client = KafkaClient::new(properties(&[("ssl.ca.location", &ca)])).with_bearer(source.clone());
    reads_through(SASL_SSL, client).await;
    assert!(*source.0.lock().unwrap() >= 1, "librdkafka asked the source for its token");
}
