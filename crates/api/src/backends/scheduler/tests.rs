//! Against the Postgres at `DATABASE_URL` (`AGENTD_PG_TESTS=1`) and the
//! Temporal at `AGENTD_TEMPORAL_ADDRESS`; skipped without them.

use diesel::sql_types::Text;
use diesel_async::RunQueryDsl;
use shared::ports::{FireTarget, ScheduleSpec, TenantRef};
use store_postgres::PgFireClaims;

use super::*;

#[derive(diesel::QueryableByName)]
struct Found {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    n: i64,
}

/// A due hourly schedule of the built-in scheduler; how many of its occurrences are queued after `wait`.
async fn queued_after(pg: &PgBackend, target: &FireTarget, wait: Duration) -> i64 {
    tokio::time::sleep(wait).await;
    let mut conn = pg.diesel.get().await.unwrap();
    let found: Found = diesel::sql_query("SELECT count(*) AS n FROM fire_claims WHERE target = $1")
        .bind::<Text, _>(target.key())
        .get_result(&mut conn)
        .await
        .unwrap();
    found.n
}

async fn due_schedule(pg: &PgBackend, tag: &str) -> ScheduleSpec {
    let spec = ScheduleSpec {
        cron: "0 * * * *".into(),
        timezone: "UTC".into(),
        target: FireTarget::new("sinktest", format!("{tag}{}", chrono::Utc::now().timestamp_micros())),
        tenant: TenantRef {
            org_id: "org_k".into(),
            workspace_id: "wrkspc_k".into(),
            api_key_id: "apikey_k".into(),
        },
    };
    PgScheduler::new(pg.clone()).upsert(spec.clone()).await.unwrap();
    let mut conn = pg.diesel.write().await.unwrap();
    diesel::sql_query("UPDATE schedules SET next_fire_at = date_trunc('hour', now()) WHERE target = $1")
        .bind::<Text, _>(spec.target.key())
        .execute(&mut conn)
        .await
        .unwrap();
    spec
}

#[tokio::test]
async fn with_temporal_the_built_in_tick_does_not_run_and_with_postgres_it_does() {
    let (Ok(url), Ok(address)) = (std::env::var("DATABASE_URL"), std::env::var("AGENTD_TEMPORAL_ADDRESS"))
    else {
        return;
    };
    if std::env::var("AGENTD_PG_TESTS").as_deref() != Ok("1") {
        return;
    }
    let pg = PgBackend::connect(&url).await.unwrap();
    let fires = Arc::new(FireRegistry::new(Arc::new(PgFireClaims::new(pg.clone()))));
    let temporal = scheduler_temporal::TemporalConfig {
        address,
        namespace: "default".into(),
        task_queue: format!("agentd-sink-test-{}", chrono::Utc::now().timestamp_micros()),
    };
    let spec = due_schedule(&pg, "temporal").await;
    let sink = Sink::connect(&SchedulerConfig::Temporal(temporal), &pg).await.unwrap();
    assert!(matches!(sink, Sink::Temporal(_)));
    sink.start(&fires, |_| {});
    assert_eq!(queued_after(&pg, &spec.target, Duration::from_secs(3)).await, 0, "no built-in tick");

    let sink = Sink::connect(&SchedulerConfig::Postgres, &pg).await.unwrap();
    sink.start(&fires, |_| {});
    assert_eq!(queued_after(&pg, &spec.target, Duration::from_secs(3)).await, 1, "the tick ran at once");
    PgScheduler::new(pg).delete(&spec.target).await.unwrap();
}
