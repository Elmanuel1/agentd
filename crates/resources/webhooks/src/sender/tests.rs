//! The wake, the claim loop and the send path against in-memory ports and a
//! local receiver. The queue's SQL is covered against Postgres in
//! `api/tests/webhooks.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::json;
use shared::Timestamp;
use shared::ports::broadcast::LocalBroadcast;
use shared::ports::changes::ChangeHandler;

use super::*;
use crate::queue::{Attempted, ClaimRef, Claimed, NativeDeliveryQueue, Next};
use crate::sign::{SigningSecret, sign};
use crate::states::{Configured, DisabledReason, EndpointState, NativeEndpointStates};

#[derive(Default)]
struct Queue {
    due: Mutex<Vec<Claimed>>,
    limits: Mutex<Vec<(usize, Vec<String>)>>,
    released: Mutex<Vec<String>>,
    dropped: Mutex<Vec<(String, String)>>,
    recorded: Mutex<Vec<(String, Attempted)>>,
}

impl NativeDeliveryQueue for Queue {
    async fn claim(&self, limit: usize, skip: &[String]) -> Result<Vec<Claimed>, BackendError> {
        self.limits.lock().push((limit, skip.to_vec()));
        let mut due = self.due.lock();
        let n = due.len().min(limit);
        Ok(due.drain(..n).collect())
    }
    async fn release(&self, c: &ClaimRef) -> Result<bool, BackendError> {
        self.released.lock().push(c.webhook_id.clone());
        Ok(true)
    }
    async fn drop_unsent(&self, c: &ClaimRef, why: &str) -> Result<bool, BackendError> {
        self.dropped.lock().push((c.webhook_id.clone(), why.to_owned()));
        Ok(true)
    }
    async fn record(&self, c: &ClaimRef, a: &Attempted) -> Result<bool, BackendError> {
        self.recorded.lock().push((c.webhook_id.clone(), a.clone()));
        Ok(true)
    }
    async fn oldest_due(&self) -> Result<Option<Timestamp>, BackendError> {
        Ok(None)
    }
}

#[derive(Default)]
struct States {
    rows: Mutex<Vec<EndpointState>>,
    disabled: Mutex<Vec<(String, DisabledReason)>>,
    failures: AtomicUsize,
}

impl NativeEndpointStates for States {
    async fn configure(&self, _: &[Configured]) -> Result<(), BackendError> {
        Ok(())
    }
    async fn read(&self) -> Result<Vec<EndpointState>, BackendError> {
        Ok(self.rows.lock().clone())
    }
    async fn failed(&self, _: &Configured) -> Result<bool, BackendError> {
        self.failures.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }
    async fn disable(&self, e: &Configured, why: DisabledReason) -> Result<bool, BackendError> {
        self.disabled.lock().push((e.endpoint_key.clone(), why));
        Ok(true)
    }
    async fn clear_failing(&self, _: &str, _: Timestamp) -> Result<(), BackendError> {
        Ok(())
    }
}

async fn receiver(code: StatusCode) -> (String, Arc<Mutex<Vec<HeaderMap>>>, std::net::SocketAddr) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let seen = got.clone();
    let app = axum::Router::new().route(
        "/hook",
        post(move |h: HeaderMap| async move {
            seen.lock().push(h);
            code
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (format!("http://{addr}/hook"), got, addr)
}

fn hooks(url: &str) -> Arc<Webhooks> {
    let json = json!([{"id": "a", "organization_id": "org_1", "url": url, "secret": "whsec_c2VjcmV0"}]);
    Arc::new(Webhooks::parse(&json.to_string()).unwrap())
}

fn due(id: &str, url: &str, attempt: i64) -> Claimed {
    let claim = ClaimRef { webhook_id: id.into(), endpoint_key: "a".into(), claim_seq: 1 };
    Claimed { claim, attempt, endpoint_url: url.into(), body: "{}".into() }
}

struct Rig {
    queue: Arc<Queue>,
    states: Arc<States>,
    wakes: Arc<LocalBroadcast>,
    stop: watch::Sender<bool>,
    running: tokio::task::JoinHandle<()>,
}

/// A sender to `url` that may reach `allowed`, running until the rig stops it.
fn start(url: &str, allowed: Vec<std::net::SocketAddr>, rows: Vec<Claimed>) -> Rig {
    start_with(url, allowed, rows, Vec::new())
}

fn start_with(
    url: &str,
    allowed: Vec<std::net::SocketAddr>,
    rows: Vec<Claimed>,
    read: Vec<EndpointState>,
) -> Rig {
    let (queue, states) = (Arc::new(Queue::default()), Arc::new(States::default()));
    *queue.due.lock() = rows;
    *states.rows.lock() = read;
    let wakes = Arc::new(LocalBroadcast::default());
    let ports = SenderPorts {
        queue: queue.clone(),
        states: states.clone(),
        wakes: wakes.clone(),
        metrics: Arc::new(NoMetrics),
    };
    let sender = WebhookSender::new(hooks(url), &Guard::allowing(allowed), ports);
    let (stop, stopping) = watch::channel(false);
    let running = tokio::spawn(sender.run(stopping));
    Rig { queue, states, wakes, stop, running }
}

impl Rig {
    async fn until(&self, done: impl Fn(&Queue) -> bool) {
        for _ in 0..300 {
            if done(&self.queue) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the sender did not finish");
    }

    async fn stop(self) {
        self.stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), self.running).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn a_new_row_wakes_the_senders_and_its_updates_do_not() {
    let wakes = Arc::new(LocalBroadcast::default());
    let mut heard = Broadcast::subscribe(wakes.as_ref(), WAKE);
    let handler = DeliveryWake::new(wakes);
    let mut change = Change {
        table: "webhook_deliveries".into(),
        op: ChangeOp::Update,
        position: None,
        before: None,
        after: Some(json!({"webhook_id": "whe_1"})),
    };
    ChangeHandler::handle(&handler, &change).await.unwrap();
    change.op = ChangeOp::Insert;
    ChangeHandler::handle(&handler, &change).await.unwrap();
    assert_eq!(heard.next().await.as_deref(), Some(""));
    assert!(tokio::time::timeout(Duration::from_millis(50), heard.next()).await.is_err(), "one wake");
}

#[tokio::test]
async fn a_claimed_delivery_is_signed_and_sent_with_live_s_headers() {
    let (url, got, addr) = receiver(StatusCode::OK).await;
    let rig = start(&url, vec![addr], vec![due("whe_1", &url, 0)]);
    rig.until(|q| !q.recorded.lock().is_empty()).await;
    let h = got.lock()[0].clone();
    assert_eq!(
        (h["user-agent"].to_str().unwrap(), h["webhook-id"].to_str().unwrap()),
        ("Claude-Platform", "whe_1")
    );
    let ts: i64 = h["webhook-timestamp"].to_str().unwrap().parse().unwrap();
    let secret = SigningSecret::parse("whsec_c2VjcmV0").unwrap();
    assert_eq!(h["webhook-signature"].to_str().unwrap(), sign(&secret, "whe_1", ts, "{}"));
    let recorded = rig.queue.recorded.lock().clone();
    let delivered = Attempted { next: Next::Delivered, status: Some(200), error: None };
    assert_eq!(recorded, [("whe_1".to_owned(), delivered)]);
    assert_eq!(rig.queue.limits.lock()[0], (100, vec![]), "one claim takes at most 100");
    rig.stop().await;
}

#[tokio::test]
async fn a_refused_attempt_waits_by_its_backoff_and_the_third_drops() {
    let (url, _, addr) = receiver(StatusCode::INTERNAL_SERVER_ERROR).await;
    let rig = start(&url, vec![addr], vec![due("whe_1", &url, 0), due("whe_2", &url, 2)]);
    rig.until(|q| q.recorded.lock().len() == 2).await;
    let recorded: HashMap<String, Attempted> = rig.queue.recorded.lock().iter().cloned().collect();
    let first = &recorded["whe_1"];
    assert!(matches!(first.next, Next::Retry(w) if (15..=30).contains(&w.as_secs())), "{first:?}");
    assert_eq!((first.status, first.error.as_deref()), (Some(500), Some("answered 500")));
    assert_eq!(recorded["whe_2"].next, Next::Dropped);
    assert_eq!(rig.states.failures.load(Ordering::SeqCst), 2, "each failure counts on the endpoint");
    rig.stop().await;
}

#[test]
fn the_second_wait_is_30_to_60_s() {
    for _ in 0..100 {
        assert!(matches!(send::retry_or_drop(2), Next::Retry(w) if (30..=60).contains(&w.as_secs())));
        assert!(matches!(send::retry_or_drop(1), Next::Retry(w) if (15..=30).contains(&w.as_secs())));
    }
    assert_eq!(send::retry_or_drop(3), Next::Dropped);
}

#[tokio::test]
async fn a_private_address_is_not_sent_to_and_disables_its_endpoint() {
    let (url, got, _) = receiver(StatusCode::OK).await;
    let rig = start(&url, vec![], vec![due("whe_1", &url, 0)]);
    rig.until(|q| !q.dropped.lock().is_empty()).await;
    assert!(got.lock().is_empty(), "nothing is posted");
    assert_eq!(rig.states.disabled.lock().as_slice(), [("a".to_owned(), DisabledReason::PrivateAddress)]);
    rig.stop().await;
}

#[tokio::test]
async fn a_redirect_disables_its_endpoint_at_once() {
    let (url, _, addr) = receiver(StatusCode::FOUND).await;
    let rig = start(&url, vec![addr], vec![due("whe_1", &url, 0)]);
    rig.until(|q| !q.recorded.lock().is_empty()).await;
    assert_eq!(rig.states.disabled.lock().as_slice(), [("a".to_owned(), DisabledReason::Redirect)]);
    rig.stop().await;
}

#[tokio::test]
async fn a_delivery_whose_claims_lapsed_three_times_or_whose_endpoint_is_disabled_drops_unsent() {
    let (url, got, addr) = receiver(StatusCode::OK).await;
    let rig = start(&url, vec![addr], vec![due("whe_3", &url, 3)]);
    rig.until(|q| !q.dropped.lock().is_empty()).await;
    let disabled = EndpointState {
        endpoint_key: "a".into(),
        failing_since: None,
        disabled: Some(DisabledReason::Failing),
    };
    let rig2 = start_with(&url, vec![addr], vec![due("whe_4", &url, 0)], vec![disabled]);
    rig2.until(|q| !q.dropped.lock().is_empty() || !q.recorded.lock().is_empty()).await;
    assert!(got.lock().is_empty());
    assert_eq!(rig.queue.dropped.lock()[0].1, "its claims lapsed three times");
    assert_eq!(
        rig2.queue.dropped.lock().as_slice(),
        [("whe_4".to_owned(), "the endpoint is disabled".to_owned())]
    );
    rig.stop().await;
    rig2.stop().await;
}

#[tokio::test]
async fn wakes_fold_into_at_most_one_claim_every_50_ms() {
    let (url, _, addr) = receiver(StatusCode::OK).await;
    let rig = start(&url, vec![addr], vec![]);
    rig.until(|q| !q.limits.lock().is_empty()).await;
    let before = rig.queue.limits.lock().len();
    let started = tokio::time::Instant::now();
    for _ in 0..200 {
        Broadcast::publish(rig.wakes.as_ref(), WAKE, "").await.unwrap();
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let claims = rig.queue.limits.lock().len() - before;
    let most = started.elapsed().as_millis() / 50 + 2;
    assert!((1..=most as usize).contains(&claims), "{claims} claims in {:?}", started.elapsed());
    rig.stop().await;
}

/// States whose read waits `delay` and fails while `failing`.
#[derive(Default)]
struct SlowStates {
    inner: States,
    delay: Mutex<Duration>,
    failing: std::sync::atomic::AtomicBool,
}

impl NativeEndpointStates for SlowStates {
    async fn configure(&self, e: &[Configured]) -> Result<(), BackendError> {
        NativeEndpointStates::configure(&self.inner, e).await
    }
    async fn read(&self) -> Result<Vec<EndpointState>, BackendError> {
        let delay = *self.delay.lock();
        tokio::time::sleep(delay).await;
        if self.failing.load(Ordering::SeqCst) {
            return Err(BackendError::new(std::io::Error::other("down")));
        }
        NativeEndpointStates::read(&self.inner).await
    }
    async fn failed(&self, e: &Configured) -> Result<bool, BackendError> {
        NativeEndpointStates::failed(&self.inner, e).await
    }
    async fn disable(&self, e: &Configured, why: DisabledReason) -> Result<bool, BackendError> {
        NativeEndpointStates::disable(&self.inner, e, why).await
    }
    async fn clear_failing(&self, k: &str, s: Timestamp) -> Result<(), BackendError> {
        NativeEndpointStates::clear_failing(&self.inner, k, s).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_or_late_state_read_stops_posts_once_the_last_good_read_is_11_s_old() {
    let states = Arc::new(SlowStates::default());
    let health = health::Health::new(states.clone());
    assert!(!health.fresh(), "nothing is sent before the first read");
    health.refresh().await.unwrap();
    assert!(health.fresh());
    states.failing.store(true, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(health.refresh().await.is_err());
    assert!(health.fresh(), "the last good read is 10 s old");
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(!health.fresh(), "11 s old: no POST starts");
    states.failing.store(false, Ordering::SeqCst);
    health.refresh().await.unwrap();
    assert!(health.fresh(), "a good read sends again");
}

#[tokio::test(start_paused = true)]
async fn a_read_that_started_before_a_disable_and_completes_11_s_later_is_discarded() {
    let states = Arc::new(SlowStates::default());
    let health = health::Health::new(states.clone());
    *states.delay.lock() = Duration::from_secs(11);
    assert!(health.refresh().await.is_err(), "discarded");
    assert!(!health.fresh());
    states.inner.rows.lock().push(EndpointState {
        endpoint_key: "a".into(),
        failing_since: None,
        disabled: Some(DisabledReason::Redirect),
    });
    *states.delay.lock() = Duration::ZERO;
    health.refresh().await.unwrap();
    assert!(health.fresh() && health.disabled("a"));
}

#[tokio::test]
async fn a_delivery_with_under_15_s_of_claim_left_is_released_unsent() {
    let (url, got, addr) = receiver(StatusCode::OK).await;
    let (queue, states) = (Arc::new(Queue::default()), Arc::new(States::default()));
    let health = Arc::new(health::Health::new(states));
    health.refresh().await.unwrap();
    let send = send::Send {
        guard: Guard::allowing(vec![addr]),
        webhooks: hooks(&url),
        queue: queue.clone(),
        health,
        metrics: Arc::new(NoMetrics),
        breakers: shared::breaker::Breakers::default(),
    };
    send.deliver(due("whe_1", &url, 1), tokio::time::Instant::now() + Duration::from_secs(14)).await;
    assert_eq!(queue.released.lock().as_slice(), ["whe_1"]);
    assert!(got.lock().is_empty() && queue.recorded.lock().is_empty(), "nothing sent, no attempt spent");
    for _ in 0..shared::breaker::FAILURES {
        send.breakers.failed(&"a".to_owned(), None);
    }
    send.deliver(due("whe_2", &url, 0), tokio::time::Instant::now() + Duration::from_secs(60)).await;
    assert_eq!(queue.released.lock().len(), 2, "an open breaker releases unsent");
    assert!(got.lock().is_empty());
}
