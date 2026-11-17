//! Resource history end to end: writes through the API reach `history.<table>`
//! through Postgres's WAL, Debezium Server and the feed (Kafka when
//! `AGENTD_KAFKA_BROKERS` is set, else Valkey Streams at `AGENTD_VALKEY_URL`).
//! Gated by `AGENTD_PG_TESTS=1`, `AGENTD_AWS_ENDPOINT` and `AGENTD_CDC_TESTS=1`
//! with a running Debezium Server (`docker/debezium`), which one part stops
//! and starts again: its container is `AGENTD_DEBEZIUM_CONTAINER` (`debezium`).
//! One test, so one set of consumers reads the feed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;
#[path = "history/kit.rs"]
mod kit;

use std::sync::Arc;

use axum::http::StatusCode;
use common::Rig;
use diesel::sql_types::{Jsonb, Text};
use diesel_async::RunQueryDsl;
use kit::{call, cdc_enabled, eventually, kept, summary, Container};
use serde_json::{json, Value};
use shared::ports::changes::{Change, ChangeHandler, ChangeOp, Position};
use shared::{with_writer, Timestamp, Writer};
use store_postgres::PgHistoryLog;

#[tokio::test]
async fn every_change_reaches_history_with_its_writer() {
    if !cdc_enabled() {
        return;
    }
    let Some(rig) = Rig::new().await else { return };
    let (st, vault, created) = call(&rig, "POST", "/v1/vaults", Some(json!({"display_name": "h"}))).await;
    assert_eq!(st, StatusCode::OK, "{vault}");
    let v = vault["id"].as_str().unwrap().to_owned();
    let (_, _, updated) =
        call(&rig, "POST", &format!("/v1/vaults/{v}"), Some(json!({"display_name": "i"}))).await;
    let (_, _, archived) = call(&rig, "POST", &format!("/v1/vaults/{v}/archive"), None).await;
    let (st, _, deleted) = call(&rig, "DELETE", &format!("/v1/vaults/{v}"), None).await;
    assert_eq!(st, StatusCode::OK);
    let agent = json!({"name": "h", "model": "claude-sonnet-4-6"});
    let (_, agent, agent_created) = call(&rig, "POST", "/v1/agents", Some(agent)).await;
    let a = agent["id"].as_str().unwrap().to_owned();
    let (_, _, agent_archived) = call(&rig, "POST", &format!("/v1/agents/{a}/archive"), None).await;

    // An update that leaves a TOASTed doc unchanged still records the whole doc; the writer is the
    // agent loop's, `system`, whose updates are kept like any other.
    let session = format!("sesn_history{}", common::stamp());
    // Random, so it does not compress below the 2 KB at which Postgres moves it out of line.
    let blob: String = (0..400).map(|_| uuid::Uuid::new_v4().simple().to_string()).collect();
    let toast = json!({"blob": blob});
    with_writer(Writer::fresh("system"), async {
        let mut conn = rig.pg.diesel.write().await.unwrap();
        diesel::sql_query(
            "INSERT INTO sessions (workspace_id, session_id, status, created_at, doc)
             VALUES ($1, $2, 'running', now(), $3)",
        )
        .bind::<Text, _>(&rig.ws)
        .bind::<Text, _>(&session)
        .bind::<Jsonb, _>(&toast)
        .execute(&mut *conn)
        .await
        .unwrap();
        diesel::sql_query("UPDATE sessions SET status = 'idle' WHERE session_id = $1")
            .bind::<Text, _>(&session)
            .execute(&mut *conn)
            .await
            .unwrap();
    })
    .await;

    // A redacted version's content leaves history; the head's current version keeps its own.
    let (_, store, _) = call(&rig, "POST", "/v1/memory_stores", Some(json!({"name": "h"}))).await;
    let sid = store["id"].as_str().unwrap().to_owned();
    let memories = format!("/v1/memory_stores/{sid}/memories");
    let (_, memory, _) =
        call(&rig, "POST", &memories, Some(json!({"path": "/a.md", "content": "one"}))).await;
    let (mid, first) =
        (memory["id"].as_str().unwrap(), memory["memory_version_id"].as_str().unwrap().to_owned());
    let (_, memory, _) =
        call(&rig, "POST", &format!("{memories}/{mid}"), Some(json!({"content": "two"}))).await;
    let second = memory["memory_version_id"].as_str().unwrap().to_owned();
    let redact = format!("/v1/memory_stores/{sid}/memory_versions/{first}/redact");
    let (st, redacted, _) = call(&rig, "POST", &redact, None).await;
    assert_eq!(st, StatusCode::OK, "{redacted}");

    let tasks = kit::record(&format!("agentd-test-{}", common::stamp())).await;

    let rows = eventually(&rig, "vaults", "vault_id", &v, 4).await;
    let writer = |op: &str, id: &str| (op.to_owned(), "apikey_01a".to_owned(), id.to_owned());
    let expected = vec![
        writer("insert", &created),
        writer("update", &updated),
        writer("update", &archived),
        writer("delete", &deleted),
    ];
    assert_eq!(summary(&rows), expected);
    assert_eq!(rows[1]["doc"]["display_name"], "i");
    assert_eq!(rows[3]["doc"]["display_name"], "i", "the deleted row as it was");

    let rows = eventually(&rig, "agents", "agent_id", &a, 2).await;
    let ops = summary(&rows);
    assert_eq!(ops[0], writer("insert", &agent_created));
    assert_eq!(ops.last(), Some(&writer("update", &agent_archived)));

    let rows = eventually(&rig, "sessions", "session_id", &session, 2).await;
    assert_eq!(summary(&rows).iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["system", "system"]);
    assert_eq!(rows[1]["doc"], toast, "the unchanged doc arrives whole");

    let versions = eventually(&rig, "memory_versions", "store_id", &sid, 3).await;
    let heads = eventually(&rig, "memories", "store_id", &sid, 2).await;
    let content = |r: &Value| (r["doc"]["content_sha256"].clone(), r["doc"]["path"].clone());
    let of = |rows: &[Value], key: &str, id: &str| -> Vec<(Value, Value)> {
        rows.iter().filter(|r| r["doc"][key] == id).map(content).collect()
    };
    let none = (Value::Null, Value::Null);
    assert_eq!(of(&versions, "id", &first), [none.clone(), none.clone()], "the redacted version, twice");
    assert_eq!(of(&heads, "memory_version_id", &first), [none], "the head that held it");
    let kept_content = of(&versions, "id", &second);
    assert!(kept_content.iter().all(|c| c.0.is_string() && c.1 == "/a.md"), "{kept_content:?}");
    assert!(of(&heads, "memory_version_id", &second).iter().all(|c| c.0.is_string()));

    // A delete while Debezium is down reaches history once it is back: the slot keeps it.
    let (_, other, _) = call(&rig, "POST", "/v1/vaults", Some(json!({"display_name": "j"}))).await;
    let o = other["id"].as_str().unwrap().to_owned();
    eventually(&rig, "vaults", "vault_id", &o, 1).await;
    let debezium = Container::named("AGENTD_DEBEZIUM_CONTAINER", "debezium");
    debezium.docker("stop");
    let (st, _, gone) = call(&rig, "DELETE", &format!("/v1/vaults/{o}"), None).await;
    assert_eq!(st, StatusCode::OK);
    debezium.docker("start");
    let rows = eventually(&rig, "vaults", "vault_id", &o, 2).await;
    assert_eq!(summary(&rows).last(), Some(&writer("delete", &gone)));

    // A change delivered twice is recorded once.
    let log = Arc::new(PgHistoryLog::load(rig.pg.clone()).await.unwrap());
    let recorder = history::HistoryRecorder::new(log, "vaults".into(), Arc::new(|_| {}));
    let twice = format!("vlt_twice{}", common::stamp());
    let change = Change {
        table: "vaults".into(),
        op: ChangeOp::Insert,
        position: Some(Position { lsn: 1, tx_id: None, committed_at: Timestamp::now() }),
        before: None,
        after: Some(json!({"workspace_id": rig.ws, "vault_id": twice, "doc": "{}"})),
    };
    recorder.handle(&change).await.unwrap();
    recorder.handle(&change).await.unwrap();
    assert_eq!(kept(&rig, "vaults", "vault_id", &twice).await.len(), 1);

    tasks.iter().for_each(tokio::task::JoinHandle::abort);
}
