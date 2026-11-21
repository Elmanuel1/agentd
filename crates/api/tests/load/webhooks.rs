//! The webhook sender under load (SH-52): 1 to 3 instances in this process,
//! each with its own pool, claim loop and wake subscription, sending to fake
//! receivers: fast (100 ms), slow (5 s) and dead (never answers), for many
//! organizations and one noisy one. Rows are inserted in batches; the change
//! feed (Kafka at `AGENTD_KAFKA_BROKERS`, else Valkey Streams at
//! `AGENTD_VALKEY_URL`, read through Debezium Server) wakes the instances,
//! and with `AGENTD_LOAD_FEED=none` only the 1 s poll does. Prints one table
//! row per run: deliveries a second, delay from insert to first attempt (p50,
//! p99), claims a second, duplicates, and the delay of the other
//! organizations while the noisy one floods.
//!
//! `AGENTD_LOAD_INSTANCES` (`1,2,3`), `AGENTD_LOAD_ORGS` (50) and
//! `AGENTD_LOAD_PER_ORG` (40) and `AGENTD_LOAD_NOISY` (4,000) size it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use diesel_async::RunQueryDsl;
use parking_lot::Mutex;
use serde_json::{Value, json};
use shared::ports::broadcast::Broadcast;
use shared::ports::changes::ChangeHandler;
use signals_valkey::{Valkey, ValkeyBroadcast, ValkeyChanges};
use store_postgres::{PgBackend, PgWebhooks, PoolSettings};
use tokio::sync::watch;
use tokio::time::Instant;
use webhooks::{DeliveryWake, SenderMetrics, SenderPorts, WebhookSender, Webhooks};

use super::common;

fn env(var: &str, default: usize) -> usize {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// First POST time per `webhook-id`, and POSTs per id.
#[derive(Default)]
struct Seen {
    first: Mutex<HashMap<String, Instant>>,
    posts: Mutex<HashMap<String, usize>>,
}

/// A receiver that answers 200 after `delay`, or never when `delay` is `None`; its URL.
async fn receiver(seen: Arc<Seen>, delay: Option<Duration>) -> (String, std::net::SocketAddr) {
    let app = axum::Router::new().route(
        "/in",
        post(move |h: HeaderMap| {
            let seen = seen.clone();
            async move {
                let id = h["webhook-id"].to_str().unwrap().to_owned();
                seen.first.lock().entry(id.clone()).or_insert_with(Instant::now);
                *seen.posts.lock().entry(id).or_insert(0) += 1;
                match delay {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
                StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (format!("http://{addr}/in"), addr)
}

#[derive(Default)]
struct Claims(AtomicUsize);

impl SenderMetrics for Claims {
    fn claimed(&self, _: usize) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn quantile(sorted: &[Duration], q: f64) -> Duration {
    let at = ((sorted.len() as f64 - 1.0) * q).round() as usize;
    sorted.get(at).copied().unwrap_or_default()
}

async fn exec(pg: &PgBackend, sql: &str) {
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query(sql).execute(&mut conn).await.unwrap();
}

/// Insert `n` deliveries to `key` at `url` in batches of 100, recording each insert's time.
async fn insert(
    pg: &PgBackend,
    key: &str,
    url: &str,
    n: usize,
    at: &Mutex<HashMap<String, Instant>>,
) -> Vec<String> {
    let s = common::stamp();
    let ids: Vec<String> = (0..n).map(|i| format!("whe_{key}_{s}_{i}")).collect();
    for batch in ids.chunks(100) {
        let mut conn = pg.diesel.get().await.unwrap();
        let now = Instant::now();
        diesel::sql_query(
            "INSERT INTO webhook_deliveries (webhook_id, endpoint_key, endpoint_url, body, next_attempt_at)
             SELECT w, $2, $3, '{}', now() FROM unnest($1::text[]) AS w",
        )
        .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(batch)
        .bind::<diesel::sql_types::Text, _>(key)
        .bind::<diesel::sql_types::Text, _>(url)
        .execute(&mut conn)
        .await
        .unwrap();
        at.lock().extend(batch.iter().map(|id| (id.clone(), now)));
    }
    ids
}

#[tokio::test]
#[ignore = "a load test, run by hand"]
async fn the_sender_scales_with_instances() {
    if !common::enabled() {
        return;
    }
    let url = std::env::var("DATABASE_URL").unwrap();
    let valkey = Valkey::open(&std::env::var("AGENTD_VALKEY_URL").unwrap(), false).unwrap();
    let feed =
        std::env::var("AGENTD_LOAD_FEED").unwrap_or_else(|_| match std::env::var("AGENTD_KAFKA_BROKERS") {
            Ok(_) => "kafka".into(),
            Err(_) => "valkey".into(),
        });
    let counts: Vec<usize> = std::env::var("AGENTD_LOAD_INSTANCES")
        .unwrap_or_else(|_| "1,2,3".into())
        .split(',')
        .filter_map(|n| n.parse().ok())
        .collect();
    let (orgs, per_org, noisy) =
        (env("AGENTD_LOAD_ORGS", 50), env("AGENTD_LOAD_PER_ORG", 40), env("AGENTD_LOAD_NOISY", 4_000));
    println!(
        "| feed | instances | deliveries | delivered/s | first attempt p50 | p99 | others' p99 while noisy | claims/s | sent twice |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for instances in counts {
        let pg = PgBackend::connect_with(&url, PoolSettings { max_size: 16, ..PoolSettings::default() })
            .await
            .unwrap();
        exec(&pg, "UPDATE webhook_deliveries SET dropped_at = now() WHERE delivered_at IS NULL AND dropped_at IS NULL").await;
        let seen = Arc::new(Seen::default());
        let (fast, fast_addr) = receiver(seen.clone(), Some(Duration::from_millis(100))).await;
        let (slow, slow_addr) = receiver(seen.clone(), Some(Duration::from_secs(5))).await;
        let (dead, dead_addr) = receiver(seen.clone(), None).await;
        let mut endpoints: Vec<Value> = (0..orgs)
            .map(|o| {
                let url = match o % 10 {
                    0 => &slow,
                    1 => &dead,
                    _ => &fast,
                };
                json!({"id": format!("ep_{o}"), "organization_id": format!("org_{o}"), "url": url, "secret": "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw"})
            })
            .collect();
        endpoints.push(json!({"id": "ep_noisy", "organization_id": "org_noisy", "url": fast, "secret": "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw"}));
        let hooks = Arc::new(Webhooks::parse(&Value::Array(endpoints).to_string()).unwrap());
        let (stop, stopping) = watch::channel(false);
        let claims = Arc::new(Claims::default());
        let mut running = Vec::new();
        let mut feeds = Vec::new();
        for i in 0..instances {
            let pool =
                PgBackend::connect_with(&url, PoolSettings { max_size: 16, ..PoolSettings::default() })
                    .await
                    .unwrap();
            let store = Arc::new(PgWebhooks::new(pool));
            let wakes: Arc<dyn Broadcast> = Arc::new(ValkeyBroadcast::connect(&valkey).unwrap());
            let ports = SenderPorts {
                queue: store.clone(),
                states: store,
                wakes: wakes.clone(),
                metrics: claims.clone(),
            };
            let guard = shared::net::Guard::allowing(vec![fast_addr, slow_addr, dead_addr]);
            running
                .push(tokio::spawn(WebhookSender::new(hooks.clone(), &guard, ports).run(stopping.clone())));
            let wake: Arc<dyn ChangeHandler> = Arc::new(DeliveryWake::new(wakes));
            let consumer = format!("load-{i}-{}", common::stamp());
            feeds.extend(match feed.as_str() {
                "kafka" => changes_kafka::KafkaChanges::new(
                    &std::env::var("AGENTD_KAFKA_BROKERS").unwrap(),
                    "agentd",
                )
                .spawn(vec![wake]),
                "valkey" => ValkeyChanges::new(&valkey, "agentd", &consumer).spawn(vec![wake]),
                _ => Vec::new(),
            });
        }
        tokio::time::sleep(Duration::from_secs(12)).await;
        let inserted = Mutex::new(HashMap::new());
        let started = Instant::now();
        let mut fast_ids = Vec::new();
        let noisy_ids = {
            let (flood_pg, flood_url) = (pg.clone(), fast.clone());
            let flood = Arc::new(Mutex::new(HashMap::new()));
            let task = tokio::spawn(async move {
                (insert(&flood_pg, "ep_noisy", &flood_url, noisy, &flood).await, flood)
            });
            for o in 0..orgs {
                let url = match o % 10 {
                    0 => &slow,
                    1 => &dead,
                    _ => &fast,
                };
                let ids = insert(&pg, &format!("ep_{o}"), url, per_org, &inserted).await;
                if o % 10 > 1 {
                    fast_ids.extend(ids);
                }
            }
            let (ids, flood) = task.await.unwrap();
            inserted.lock().extend(flood.lock().drain());
            ids
        };
        let total = orgs * per_org + noisy;
        let deadline = Instant::now() + Duration::from_secs(600);
        let answered = |ids: &[String]| ids.iter().filter(|id| seen.first.lock().contains_key(*id)).count();
        while (answered(&fast_ids) < fast_ids.len() || answered(&noisy_ids) < noisy_ids.len())
            && Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let elapsed = started.elapsed();
        let delays = |ids: &[String]| {
            let (first, at) = (seen.first.lock(), inserted.lock());
            let mut d: Vec<Duration> = ids
                .iter()
                .filter_map(|id| Some(first.get(id)?.saturating_duration_since(*at.get(id)?)))
                .collect();
            d.sort();
            d
        };
        let all: Vec<String> = fast_ids.iter().chain(&noisy_ids).cloned().collect();
        let (every, others) = (delays(&all), delays(&fast_ids));
        let twice = seen.posts.lock().iter().filter(|(id, n)| **n > 1 && all.contains(id)).count();
        let delivered = all.len() as f64 / elapsed.as_secs_f64();
        println!(
            "| {feed} | {instances} | {total} | {delivered:.0} | {} ms | {} ms | {} ms | {:.1} | {twice} |",
            quantile(&every, 0.5).as_millis(),
            quantile(&every, 0.99).as_millis(),
            quantile(&others, 0.99).as_millis(),
            claims.0.load(Ordering::Relaxed) as f64 / elapsed.as_secs_f64(),
        );
        assert_eq!(twice, 0, "no delivery to a live receiver is sent twice");
        stop.send(true).unwrap();
        feeds.iter().for_each(tokio::task::JoinHandle::abort);
        for r in running {
            r.abort();
        }
    }
}
