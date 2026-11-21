//! Sending from the rows (SH-1 to SH-4, SH-29 to SH-34, SH-36, SH-52): the
//! claim and its fences on Postgres, and senders over it. Every test claims
//! from the whole table, so they take turns, and each first drops the rows
//! earlier binaries left due.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use diesel_async::RunQueryDsl;
use parking_lot::Mutex;
use serde_json::{Value, json};
use shared::ports::broadcast::LocalBroadcast;
use shared::ports::changes::{Change, ChangeHandler, ChangeOp};
use store_postgres::{PgBackend, PgWebhooks, PoolSettings};
use tokio::sync::watch;
use webhooks::queue::{Attempted, DeliveryQueue, Next};
use webhooks::states::{Configured, DisabledReason, EndpointStates};
use webhooks::{DeliveryWake, SenderMetrics, SenderPorts, WebhookSender, Webhooks};

use super::common;

static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Db {
    pg: PgBackend,
    store: Arc<PgWebhooks>,
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

async fn db() -> Option<Db> {
    if !common::enabled() {
        return None;
    }
    let turn = TURN.lock().await;
    let url = std::env::var("DATABASE_URL").unwrap();
    let pg = PgBackend::connect_with(&url, PoolSettings { max_size: 24, ..PoolSettings::default() })
        .await
        .unwrap();
    exec(
        &pg,
        "UPDATE webhook_deliveries SET dropped_at = now() WHERE delivered_at IS NULL AND dropped_at IS NULL",
    )
    .await;
    Some(Db { store: Arc::new(PgWebhooks::new(pg.clone())), pg, _turn: turn })
}

async fn exec(pg: &PgBackend, sql: &str) {
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query(sql).execute(&mut conn).await.unwrap();
}

/// `n` due deliveries to `key` at `url`; their ids.
async fn insert(pg: &PgBackend, key: &str, url: &str, n: usize) -> Vec<String> {
    let s = common::stamp();
    let ids: Vec<String> = (0..n).map(|i| format!("whe_{s}_{i}")).collect();
    let mut conn = pg.diesel.get().await.unwrap();
    diesel::sql_query(
        "INSERT INTO webhook_deliveries (webhook_id, endpoint_key, endpoint_url, body, next_attempt_at)
         SELECT w, $2, $3, '{}', now() FROM unnest($1::text[]) AS w",
    )
    .bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(&ids)
    .bind::<diesel::sql_types::Text, _>(key)
    .bind::<diesel::sql_types::Text, _>(url)
    .execute(&mut conn)
    .await
    .unwrap();
    ids
}

#[derive(diesel::QueryableByName)]
struct Row {
    #[diesel(sql_type = diesel::sql_types::Jsonb)]
    row: Value,
}

async fn row(pg: &PgBackend, id: &str) -> Value {
    let mut conn = pg.diesel.get().await.unwrap();
    let q = diesel::sql_query(
        "SELECT to_jsonb(d) || jsonb_build_object('wait', extract(epoch FROM next_attempt_at - now())) AS row
         FROM webhook_deliveries d WHERE webhook_id = $1",
    )
    .bind::<diesel::sql_types::Text, _>(id);
    let r: Row = q.get_result(&mut conn).await.unwrap();
    r.row
}

async fn expire(pg: &PgBackend, id: &str) {
    exec(pg, &format!("UPDATE webhook_deliveries SET claimed_until = now() - interval '1 second' WHERE webhook_id = '{id}'"))
        .await;
}

/// A receiver answering `code` after `delay`; the POSTs it got, by `webhook-id`.
async fn receiver(
    code: StatusCode,
    delay: Duration,
) -> (String, Arc<Mutex<HashMap<String, usize>>>, std::net::SocketAddr) {
    let got = Arc::new(Mutex::new(HashMap::new()));
    let seen = got.clone();
    let app = axum::Router::new().route(
        "/in",
        post(move |h: HeaderMap| async move {
            let id = h["webhook-id"].to_str().unwrap().to_owned();
            *seen.lock().entry(id).or_insert(0) += 1;
            tokio::time::sleep(delay).await;
            code
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (format!("http://{addr}/in"), got, addr)
}

fn hooks(endpoints: &[(&str, &str)]) -> Arc<Webhooks> {
    let list: Vec<Value> = endpoints
        .iter()
        .map(|(k, url)| json!({"id": k, "organization_id": "org_hooks", "url": url, "secret": "whsec_c2VjcmV0"}))
        .collect();
    Arc::new(Webhooks::parse(&Value::Array(list).to_string()).unwrap())
}

#[derive(Default)]
struct Claims(AtomicUsize);

impl SenderMetrics for Claims {
    fn claimed(&self, n: usize) {
        self.0.fetch_add(n, Ordering::SeqCst);
    }
}

struct Running {
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    claims: Arc<Claims>,
    wakes: Arc<LocalBroadcast>,
}

/// One instance's sender over `db`, reaching `allowed`.
fn start(db: &Db, hooks: Arc<Webhooks>, allowed: Vec<std::net::SocketAddr>) -> Running {
    let (claims, wakes) = (Arc::new(Claims::default()), Arc::new(LocalBroadcast::default()));
    let ports = SenderPorts {
        queue: db.store.clone(),
        states: db.store.clone(),
        wakes: wakes.clone(),
        metrics: claims.clone(),
    };
    let sender = WebhookSender::new(hooks, &shared::net::Guard::allowing(allowed), ports);
    let (stop, stopping) = watch::channel(false);
    Running { stop, task: tokio::spawn(sender.run(stopping)), claims, wakes }
}

impl Running {
    async fn stop(self) {
        self.stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(15), self.task).await.unwrap().unwrap();
    }
}

async fn until(what: &str, done: impl AsyncFn() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !done().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn configured(key: &str, digest: &str) -> Configured {
    Configured { endpoint_key: key.into(), org_id: "org_hooks".into(), digest: digest.into() }
}

#[tokio::test]
async fn twenty_claimers_started_together_claim_every_row_once() {
    let Some(db) = db().await else { return };
    let key = format!("ep_{}", common::stamp());
    for round in 0..50 {
        let ids = insert(&db.pg, &key, "https://hooks.agentd.test/in", 1_000).await;
        let start = Arc::new(tokio::sync::Barrier::new(20));
        let claimers = (0..20).map(|_| {
            let (store, start) = (db.store.clone(), start.clone());
            tokio::spawn(async move {
                start.wait().await;
                let mut mine = Vec::new();
                loop {
                    let got = store.claim(100, &[]).await.unwrap();
                    if got.is_empty() {
                        return mine;
                    }
                    mine.extend(got.into_iter().map(|c| c.claim.webhook_id));
                }
            })
        });
        let mut all: Vec<String> = Vec::new();
        for c in claimers {
            all.extend(c.await.unwrap());
        }
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!((n, all.len()), (ids.len(), ids.len()), "round {round}: claimed once each");
        exec(
            &db.pg,
            &format!("UPDATE webhook_deliveries SET dropped_at = now() WHERE endpoint_key = '{key}'"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_live_claim_is_not_taken_over_an_expired_one_is_and_a_locked_row_is_skipped() {
    let Some(db) = db().await else { return };
    let ids = insert(&db.pg, "ep_live", "https://hooks.agentd.test/in", 1).await;
    let first = db.store.claim(10, &[]).await.unwrap();
    assert_eq!((first.len(), first[0].claim.claim_seq, first[0].attempt), (1, 1, 0));
    assert!(db.store.claim(10, &[]).await.unwrap().is_empty(), "a live claim is not taken over");
    expire(&db.pg, &ids[0]).await;
    let second = db.store.claim(10, &[]).await.unwrap();
    assert_eq!((second[0].claim.claim_seq, second[0].attempt), (2, 1), "a lapsed claim counts as an attempt");

    // A row another claim's transaction has locked between its select and update is skipped.
    expire(&db.pg, &ids[0]).await;
    let mut held = db.pg.diesel.get().await.unwrap();
    diesel::sql_query("BEGIN").execute(&mut held).await.unwrap();
    diesel::sql_query(format!(
        "SELECT webhook_id FROM webhook_deliveries WHERE webhook_id = '{}' FOR NO KEY UPDATE",
        ids[0]
    ))
    .execute(&mut held)
    .await
    .unwrap();
    assert!(db.store.claim(10, &[]).await.unwrap().is_empty(), "the locked row is skipped");
    diesel::sql_query("ROLLBACK").execute(&mut held).await.unwrap();
}

#[tokio::test]
async fn releases_spend_no_attempt_and_a_replaced_claim_writes_nothing() {
    let Some(db) = db().await else { return };
    let ids = insert(&db.pg, "ep_fence", "https://hooks.agentd.test/in", 1).await;
    let mut last = None;
    for seq in 1..=3 {
        let c = db.store.claim(10, &[]).await.unwrap().remove(0);
        assert_eq!((c.claim.claim_seq, c.attempt), (seq, 0), "three released claims spend no attempt");
        assert!(db.store.release(&c.claim).await.unwrap());
        last = Some(c);
    }
    let released = last.unwrap();
    let newer = db.store.claim(10, &[]).await.unwrap().remove(0);
    assert!(newer.claim.claim_seq > released.claim.claim_seq, "a re-claim gets a higher claim_seq");
    let delivered = Attempted { next: Next::Delivered, status: Some(200), error: None };
    assert!(
        !db.store.record(&released.claim, &delivered).await.unwrap(),
        "the released claim's outcome changes nothing"
    );
    expire(&db.pg, &ids[0]).await;
    let newest = db.store.claim(10, &[]).await.unwrap().remove(0);
    assert!(!db.store.release(&newer.claim).await.unwrap(), "an expired claim's release changes nothing");
    assert!(!db.store.record(&newer.claim, &delivered).await.unwrap());
    let r = row(&db.pg, &ids[0]).await;
    assert!(r["delivered_at"].is_null() && r["claimed_until"].is_string());
    assert_eq!(r["claim_seq"], newest.claim.claim_seq);
}

#[tokio::test]
async fn a_late_outcome_from_an_expired_claim_before_any_re_claim_is_recorded() {
    let Some(db) = db().await else { return };
    let ids = insert(&db.pg, "ep_late", "https://hooks.agentd.test/in", 1).await;
    let c = db.store.claim(10, &[]).await.unwrap().remove(0);
    expire(&db.pg, &ids[0]).await;
    let delivered = Attempted { next: Next::Delivered, status: Some(204), error: None };
    assert!(db.store.record(&c.claim, &delivered).await.unwrap());
    assert!(db.store.claim(10, &[]).await.unwrap().is_empty(), "it is not sent again");
    let r = row(&db.pg, &ids[0]).await;
    assert_eq!((r["attempt"].as_i64(), r["last_status"].as_i64()), (Some(1), Some(204)));
}

#[tokio::test]
async fn claims_that_lapse_three_times_reach_attempt_3_and_stay_there() {
    let Some(db) = db().await else { return };
    let ids = insert(&db.pg, "ep_lapse", "https://hooks.agentd.test/in", 1).await;
    for attempt in 0..=3 {
        let c = db.store.claim(10, &[]).await.unwrap().remove(0);
        assert_eq!(c.attempt, attempt);
        expire(&db.pg, &ids[0]).await;
    }
    let c = db.store.claim(10, &[]).await.unwrap().remove(0);
    assert_eq!(c.attempt, 3, "a claim at 3 that lapsed is still at 3");
    assert!(db.store.drop_unsent(&c.claim, "its claims lapsed three times").await.unwrap());
    let r = row(&db.pg, &ids[0]).await;
    assert_eq!((r["attempt"].as_i64(), r["dropped_at"].is_string()), (Some(3), true));
}

#[tokio::test]
async fn an_open_breaker_s_endpoint_is_not_claimed() {
    let Some(db) = db().await else { return };
    insert(&db.pg, "ep_open", "https://hooks.agentd.test/in", 2).await;
    assert!(db.store.claim(10, &["ep_open".to_owned()]).await.unwrap().is_empty());
    assert_eq!(db.store.claim(10, &[]).await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_new_delivery_is_sent_within_100_ms_of_its_wake_once_however_often_the_feed_hands_it_over() {
    let Some(db) = db().await else { return };
    let (url, got, addr) = receiver(StatusCode::OK, Duration::ZERO).await;
    let sender = start(&db, hooks(&[("ep_fast", &url)]), vec![addr]);
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let ids = insert(&db.pg, "ep_fast", &url, 1).await;
    let inserted = tokio::time::Instant::now();
    let wake = DeliveryWake::new(sender.wakes.clone());
    let change = Change {
        table: "webhook_deliveries".into(),
        op: ChangeOp::Insert,
        position: None,
        before: None,
        after: Some(json!({"webhook_id": ids[0]})),
    };
    ChangeHandler::handle(&wake, &change).await.unwrap();
    ChangeHandler::handle(&wake, &change).await.unwrap();
    until("the delivery", async || got.lock().contains_key(&ids[0])).await;
    assert!(inserted.elapsed() < Duration::from_millis(100), "{:?}", inserted.elapsed());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(got.lock()[&ids[0]], 1);
    until("recorded", async || row(&db.pg, &ids[0]).await["delivered_at"].is_string()).await;
    sender.stop().await;
}

#[tokio::test]
async fn three_instances_share_3000_deliveries_each_sends_some_and_none_twice() {
    let Some(db) = db().await else { return };
    let (url, got, addr) = receiver(StatusCode::OK, Duration::from_millis(5)).await;
    let ids = insert(&db.pg, "ep_share", &url, 3_000).await;
    let senders: Vec<Running> =
        (0..3).map(|_| start(&db, hooks(&[("ep_share", &url)]), vec![addr])).collect();
    until("every delivery", async || got.lock().len() == ids.len()).await;
    assert!(got.lock().values().all(|n| *n == 1), "none twice");
    for s in senders {
        assert!(s.claims.0.load(Ordering::SeqCst) > 0, "each instance sends some");
        s.stop().await;
    }
}

#[tokio::test]
async fn a_dead_endpoint_does_not_delay_another_and_a_stopped_instance_s_row_is_sent_by_another() {
    let Some(db) = db().await else { return };
    let (dead, _, dead_addr) = receiver(StatusCode::OK, Duration::from_secs(30)).await;
    let (url, got, addr) = receiver(StatusCode::OK, Duration::ZERO).await;
    insert(&db.pg, "ep_dead", &dead, 50).await;
    let left = insert(&db.pg, "ep_alive", &url, 1).await;
    // A stopped instance's claim: taken, never sent, expired.
    db.store.claim(100, &[]).await.unwrap();
    exec(&db.pg, "UPDATE webhook_deliveries SET claimed_until = now() - interval '1 second' WHERE claimed_until IS NOT NULL")
        .await;
    let sender = start(&db, hooks(&[("ep_dead", &dead), ("ep_alive", &url)]), vec![dead_addr, addr]);
    let fresh = insert(&db.pg, "ep_alive", &url, 1).await;
    until("the live endpoint's deliveries", async || {
        let got = got.lock();
        got.contains_key(&left[0]) && got.contains_key(&fresh[0])
    })
    .await;
    sender.stop.send(true).unwrap();
}

#[tokio::test]
async fn a_refused_delivery_waits_15_to_30_s_then_30_to_60_s_then_drops() {
    let Some(db) = db().await else { return };
    let (url, got, addr) = receiver(StatusCode::SERVICE_UNAVAILABLE, Duration::ZERO).await;
    let ids = insert(&db.pg, "ep_retry", &url, 1).await;
    let sender = start(&db, hooks(&[("ep_retry", &url)]), vec![addr]);
    for (attempt, (low, high)) in [(1, (14.0, 30.0)), (2, (29.0, 60.0))] {
        until("the attempt", async || row(&db.pg, &ids[0]).await["attempt"] == attempt).await;
        let r = row(&db.pg, &ids[0]).await;
        let wait = r["wait"].as_f64().unwrap();
        assert!((low..=high).contains(&wait), "attempt {attempt}: {wait}");
        assert_eq!((r["last_status"].as_i64(), r["claimed_until"].is_null()), (Some(503), true));
        exec(
            &db.pg,
            &format!("UPDATE webhook_deliveries SET next_attempt_at = now() WHERE webhook_id = '{}'", ids[0]),
        )
        .await;
    }
    until("the drop", async || row(&db.pg, &ids[0]).await["dropped_at"].is_string()).await;
    assert_eq!((row(&db.pg, &ids[0]).await["attempt"].as_i64(), got.lock()[&ids[0]]), (Some(3), 3));
    sender.stop().await;
}

#[tokio::test]
async fn two_replicas_send_a_delivery_once() {
    let Some(db) = db().await else { return };
    let (url, got, addr) = receiver(StatusCode::OK, Duration::from_millis(200)).await;
    let ids = insert(&db.pg, "ep_once", &url, 1).await;
    let (a, b) = (
        start(&db, hooks(&[("ep_once", &url)]), vec![addr]),
        start(&db, hooks(&[("ep_once", &url)]), vec![addr]),
    );
    until("recorded", async || row(&db.pg, &ids[0]).await["delivered_at"].is_string()).await;
    assert_eq!(got.lock()[&ids[0]], 1);
    a.stop().await;
    b.stop().await;
}

#[tokio::test]
async fn a_disabled_endpoint_gets_no_post_and_its_rows_drop_when_due() {
    let Some(db) = db().await else { return };
    let (url, got, addr) = receiver(StatusCode::OK, Duration::ZERO).await;
    let endpoint = configured("ep_off", "d1");
    db.store.configure(std::slice::from_ref(&endpoint)).await.unwrap();
    assert!(db.store.disable(&endpoint, DisabledReason::Redirect).await.unwrap());
    assert!(!db.store.disable(&endpoint, DisabledReason::Failing).await.unwrap(), "disabled once");
    let ids = insert(&db.pg, "ep_off", &url, 2).await;
    let sender = start(&db, hooks(&[("ep_off", &url)]), vec![addr]);
    until("the drops", async || row(&db.pg, &ids[1]).await["dropped_at"].is_string()).await;
    assert!(got.lock().is_empty());
    assert_eq!(row(&db.pg, &ids[0]).await["last_error"], "the endpoint is disabled");
    sender.stop().await;
}

#[tokio::test]
async fn a_redirect_disables_at_once_and_a_private_address_is_not_sent_to() {
    let Some(db) = db().await else { return };
    let (url, _, addr) = receiver(StatusCode::MOVED_PERMANENTLY, Duration::ZERO).await;
    let (private, got, _) = receiver(StatusCode::OK, Duration::ZERO).await;
    insert(&db.pg, "ep_moved", &url, 1).await;
    let ids = insert(&db.pg, "ep_private", &private, 1).await;
    let sender = start(&db, hooks(&[("ep_moved", &url), ("ep_private", &private)]), vec![addr]);
    until("both disabled", async || {
        let states = db.store.read().await.unwrap();
        let reason = |k: &str| states.iter().find(|s| s.endpoint_key == k).and_then(|s| s.disabled);
        reason("ep_moved") == Some(DisabledReason::Redirect)
            && reason("ep_private") == Some(DisabledReason::PrivateAddress)
    })
    .await;
    assert!(got.lock().is_empty() && row(&db.pg, &ids[0]).await["dropped_at"].is_string());
    sender.stop().await;
}

#[tokio::test]
async fn failing_for_120_h_disables_and_a_2xx_in_between_restarts_the_clock() {
    let Some(db) = db().await else { return };
    let endpoint = configured(&format!("ep_{}", common::stamp()), "d1");
    db.store.configure(std::slice::from_ref(&endpoint)).await.unwrap();
    assert!(!db.store.failed(&endpoint).await.unwrap());
    let set_since = |hours: i64| {
        format!(
            "UPDATE webhook_endpoint_states SET failing_since = now() - interval '{hours} hours' WHERE endpoint_key = '{}'",
            endpoint.endpoint_key
        )
    };
    exec(&db.pg, &set_since(119)).await;
    let since = |states: Vec<webhooks::states::EndpointState>| {
        states.into_iter().find(|s| s.endpoint_key == endpoint.endpoint_key).and_then(|s| s.failing_since)
    };
    let run = since(db.store.read().await.unwrap()).unwrap();
    db.store.clear_failing(&endpoint.endpoint_key, run).await.unwrap();
    assert_eq!(since(db.store.read().await.unwrap()), None, "a 2xx ends the run");
    assert!(!db.store.failed(&endpoint).await.unwrap(), "a new run starts now");
    exec(&db.pg, &set_since(121)).await;
    assert!(db.store.failed(&endpoint).await.unwrap(), "120 h of failures disable it");
    assert!(!db.store.failed(&endpoint).await.unwrap(), "once");
    let state = db.store.read().await.unwrap().into_iter().find(|s| s.endpoint_key == endpoint.endpoint_key);
    assert_eq!(state.unwrap().disabled, Some(DisabledReason::Failing));
}

#[tokio::test]
async fn a_changed_url_re_enables_at_boot_and_an_unchanged_config_keeps_it_disabled() {
    let Some(db) = db().await else { return };
    let key = format!("ep_{}", common::stamp());
    db.store.configure(&[configured(&key, "d1")]).await.unwrap();
    db.store.disable(&configured(&key, "d1"), DisabledReason::Redirect).await.unwrap();
    let disabled =
        async || db.store.read().await.unwrap().iter().any(|s| s.endpoint_key == key && s.disabled.is_some());
    db.store.configure(&[configured(&key, "d1")]).await.unwrap();
    assert!(disabled().await, "an unchanged config keeps it disabled");
    db.store.configure(&[configured(&key, "d2")]).await.unwrap();
    assert!(!disabled().await, "a changed URL or secret enables it");
}
