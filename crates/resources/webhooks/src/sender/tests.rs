//! In-memory ports, a local receiver and a rig that runs the sender over
//! them. The tests are in `tests/send.rs` (the wake, the claim loop and the
//! send path) and `tests/health.rs` (the endpoint states); the queue's SQL is
//! covered against Postgres in `api/tests/webhooks.rs`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use parking_lot::Mutex;
use serde_json::json;
use shared::Timestamp;
use shared::ports::broadcast::LocalBroadcast;

use super::*;
use crate::queue::{Attempted, ClaimRef, Claimed, NativeDeliveryQueue};
use crate::states::{Configured, DisabledReason, EndpointState, NativeEndpointStates};

mod health;
mod send;

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
    let json = json!([{"id": "a", "organization_id": "org_1", "url": url, "secret": "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw"}]);
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
