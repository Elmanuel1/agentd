//! What the history tests share: calls through the rig, the history rows they
//! leave, the recorders on the feed (Kafka when `AGENTD_KAFKA_BROKERS` is set,
//! else Valkey Streams at `AGENTD_VALKEY_URL`), and the containers a test
//! stops, kills or pauses.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use changes_kafka::KafkaChanges;
use diesel::sql_types::{Jsonb, Text};
use diesel_async::RunQueryDsl;
use http_body_util::BodyExt;
use serde_json::Value;
use signals_valkey::{Valkey, ValkeyChanges};
use store_postgres::{PgBackend, PgHistoryLog};
use tower::ServiceExt;

use crate::common::{request, Auth, Rig, KEY};

pub fn cdc_enabled() -> bool {
    std::env::var("AGENTD_CDC_TESTS").as_deref() == Ok("1")
}

/// Call the rig's app; the status, body and `request-id`.
pub async fn call(rig: &Rig, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value, String) {
    let resp = rig.app.clone().oneshot(request(method, uri, Auth::Key(KEY), body)).await.unwrap();
    let (status, id) = (resp.status(), resp.headers()["request-id"].to_str().unwrap().to_owned());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), id)
}

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = Jsonb)]
    row: Value,
}

/// `table`'s history rows whose `column` is `id`, in lsn order, each as
/// `{op, actor, request_id, doc}`.
pub async fn kept(rig: &Rig, table: &str, column: &str, id: &str) -> Vec<Value> {
    let mut conn = rig.pg.diesel.get().await.unwrap();
    let rows: Vec<Row> = diesel::sql_query(format!(
        "SELECT jsonb_build_object('op', op, 'actor', actor, 'request_id', request_id, 'doc', doc) AS row
           FROM history.{table} WHERE {column} = $1 ORDER BY lsn"
    ))
    .bind::<Text, _>(id)
    .load(&mut *conn)
    .await
    .unwrap();
    rows.into_iter().map(|r| r.row).collect()
}

/// `table`'s history of `id` once it holds `n` rows, or after 120 s.
pub async fn eventually(rig: &Rig, table: &str, column: &str, id: &str, n: usize) -> Vec<Value> {
    for _ in 0..240 {
        let rows = kept(rig, table, column, id).await;
        if rows.len() >= n {
            return rows;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    kept(rig, table, column, id).await
}

/// Each row's `(op, actor, request_id)`.
pub fn summary(rows: &[Value]) -> Vec<(String, String, String)> {
    let field = |r: &Value, k: &str| r[k].as_str().unwrap_or_default().to_owned();
    rows.iter().map(|r| (field(r, "op"), field(r, "actor"), field(r, "request_id"))).collect()
}

/// The recorders of every history table, reading the feed as `consumer`, on
/// a pool of their own so catching up never starves the rig's reads.
pub async fn record(consumer: &str) -> Vec<tokio::task::JoinHandle<()>> {
    let url = std::env::var("DATABASE_URL").unwrap();
    let log = PgHistoryLog::load(PgBackend::connect(&url).await.unwrap()).await.unwrap();
    let recorders = history::recorders(Arc::new(log), Arc::new(|_| {}));
    match std::env::var("AGENTD_KAFKA_BROKERS") {
        Ok(brokers) => KafkaChanges::new(&brokers, "agentd").spawn(recorders),
        Err(_) => {
            let url = std::env::var("AGENTD_VALKEY_URL").unwrap();
            let cluster = std::env::var("AGENTD_VALKEY_CLUSTER").as_deref() == Ok("1");
            let valkey = Valkey::open(&url, cluster).unwrap();
            ValkeyChanges::new(&valkey, "agentd", consumer).spawn(recorders)
        }
    }
}

/// A Docker container a test breaks, named by the environment variable
/// `var`, else `default`.
pub struct Container(String);

impl Container {
    pub fn named(var: &str, default: &str) -> Self {
        Self(std::env::var(var).unwrap_or_else(|_| default.into()))
    }

    /// `docker <verb>` it: `stop`, `start`, `kill`, `pause`, `unpause` or `restart`.
    pub fn docker(&self, verb: &str) {
        let status = std::process::Command::new("docker")
            .args([verb, &self.0])
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "docker {verb} {}", self.0);
    }
}
