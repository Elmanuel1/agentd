//! The change feed end to end: a deployment write's schedule change leaves
//! the outbox empty, and reaches the scheduler through Postgres's WAL,
//! Debezium Server and the feed: Kafka when `AGENTD_KAFKA_BROKERS` is set,
//! else Valkey Streams at `AGENTD_VALKEY_URL`. Gated by `AGENTD_PG_TESTS=1`
//! and `AGENTD_AWS_ENDPOINT`; the WAL test also needs `AGENTD_CDC_TESTS=1`
//! and a running Debezium Server (`docker/debezium`) writing to that feed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use changes_kafka::KafkaChanges;
use common::{Auth, KEY, Rig};
use deployments::fire::target;
use deployments::types::DeploymentId;
use diesel::QueryableByName;
use diesel::sql_types::{BigInt, Text};
use diesel_async::RunQueryDsl;
use serde_json::{Value, json};
use shared::ports::changes::ChangeHandler;
use shared::ports::{FireTarget, SchedulerError, SchedulerPort};
use signals_valkey::{Valkey, ValkeyChanges};
use store_postgres::{PgScheduler, ScheduleOutbox};

fn cdc_enabled() -> bool {
    std::env::var("AGENTD_CDC_TESTS").as_deref() == Ok("1")
}

/// A scheduled deployment in the rig's workspace; its id.
async fn scheduled(rig: &Rig) -> String {
    let (_, env) = rig
        .call(
            "POST",
            "/v1/environments",
            Auth::Key(KEY),
            Some(json!({"name": "cdc", "config": {"type": "self_hosted"}})),
        )
        .await;
    let (_, agent) = rig
        .call(
            "POST",
            "/v1/agents",
            Auth::Key(KEY),
            Some(json!({"name": "cdc", "model": "claude-sonnet-4-6"})),
        )
        .await;
    let body = json!({
        "name": "cdc",
        "agent": agent["id"],
        "environment_id": env["id"],
        "initial_events": [{"type": "user.message", "content": [{"type": "text", "text": "run"}]}],
        "schedule": {"type": "cron", "expression": "0 20 * * 5", "timezone": "UTC"},
    });
    let (st, depl) = rig.call("POST", "/v1/deployments", Auth::Key(KEY), Some(body)).await;
    assert_eq!(st, StatusCode::OK, "{depl}");
    depl["id"].as_str().unwrap().to_owned()
}

fn target_of(id: &str) -> FireTarget {
    target(&DeploymentId::parse(id).unwrap())
}

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

#[tokio::test]
async fn a_deployment_write_leaves_the_outbox_empty() {
    let Some(rig) = Rig::new().await else { return };
    let id = scheduled(&rig).await;
    let mut conn = rig.pg.diesel.get().await.unwrap();
    let left: Count = diesel::sql_query("SELECT count(*) AS n FROM schedule_outbox WHERE target = $1")
        .bind::<Text, _>(target_of(&id).key())
        .get_result(&mut conn)
        .await
        .unwrap();
    assert_eq!(left.n, 0, "the WAL keeps the insert; the row itself is deleted");
}

/// Until `done` holds of the scheduler's view of `target`, or 60 s pass.
async fn eventually(
    sched: &PgScheduler,
    target: &FireTarget,
    done: impl Fn(&Result<bool, SchedulerError>) -> bool,
) {
    for _ in 0..120 {
        let seen = sched.describe(target).await.map(|s| s.paused);
        if done(&seen) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the scheduler never saw the change for {}", target.key());
}

#[tokio::test]
async fn a_deployment_s_schedule_changes_reach_the_scheduler_through_the_wal() {
    if !cdc_enabled() {
        return;
    }
    let Some(rig) = Rig::new().await else { return };
    let sched = PgScheduler::new(rig.pg.clone());
    let outbox: Arc<dyn ChangeHandler> = Arc::new(ScheduleOutbox::new(Arc::new(sched.clone())));
    let tasks = match std::env::var("AGENTD_KAFKA_BROKERS") {
        Ok(brokers) => KafkaChanges::new(&brokers, "agentd").spawn(vec![outbox]),
        Err(_) => {
            let url = std::env::var("AGENTD_VALKEY_URL").unwrap();
            let cluster = std::env::var("AGENTD_VALKEY_CLUSTER").as_deref() == Ok("1");
            let valkey = Valkey::open(&url, cluster).unwrap();
            ValkeyChanges::new(&valkey, "agentd", &format!("agentd-test-{}", common::stamp()))
                .spawn(vec![outbox])
        }
    };

    let id = scheduled(&rig).await;
    let t = target_of(&id);
    eventually(&sched, &t, |s| matches!(s, Ok(false))).await;

    let (st, _): (StatusCode, Value) =
        rig.call("POST", &format!("/v1/deployments/{id}/pause"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    eventually(&sched, &t, |s| matches!(s, Ok(true))).await;

    let (st, _): (StatusCode, Value) =
        rig.call("POST", &format!("/v1/deployments/{id}/archive"), Auth::Key(KEY), None).await;
    assert_eq!(st, StatusCode::OK);
    eventually(&sched, &t, |s| matches!(s, Err(SchedulerError::Unknown(_)))).await;

    tasks.iter().for_each(tokio::task::JoinHandle::abort);
}
