//! The endpoint states: a stale or late read stops posts, and a claim near its end is released.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use parking_lot::Mutex;

use super::*;
use crate::states::{Configured, DisabledReason, EndpointState, NativeEndpointStates};

/// States whose read waits `delay` and fails while `failing`.
#[derive(Default)]
struct SlowStates {
    inner: States,
    delay: Mutex<Duration>,
    failing: std::sync::atomic::AtomicBool,
    disable_delay: Mutex<Duration>,
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
        let delay = *self.disable_delay.lock();
        tokio::time::sleep(delay).await;
        NativeEndpointStates::disable(&self.inner, e, why).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_or_late_state_read_stops_posts_once_the_last_good_read_is_11_s_old() {
    let states = Arc::new(SlowStates::default());
    let health = crate::sender::health::Health::new(states.clone());
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
    let health = crate::sender::health::Health::new(states.clone());
    *states.delay.lock() = Duration::from_secs(11);
    assert!(health.refresh().await.is_err(), "discarded");
    assert!(!health.fresh());
    states
        .inner
        .rows
        .lock()
        .push(EndpointState { endpoint_key: "a".into(), disabled: DisabledReason::Redirect });
    *states.delay.lock() = Duration::ZERO;
    health.refresh().await.unwrap();
    assert!(health.fresh() && health.disabled("a"));
}

#[tokio::test(start_paused = true)]
async fn a_disable_made_while_a_read_runs_survives_that_read() {
    let states = Arc::new(SlowStates::default());
    let health = Arc::new(crate::sender::health::Health::new(states.clone()));
    *states.delay.lock() = Duration::from_secs(2);
    let reading = tokio::spawn({
        let health = health.clone();
        async move { health.refresh().await }
    });
    tokio::time::advance(Duration::from_secs(1)).await;
    let endpoint = Configured { endpoint_key: "a".into(), org_id: "org_1".into(), digest: "d".into() };
    health.disable(&endpoint, DisabledReason::Redirect).await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    reading.await.unwrap().unwrap();
    assert!(health.disabled("a"), "the read began before the disable and did not see it");
}

#[tokio::test(start_paused = true)]
async fn a_read_that_runs_while_a_disable_is_written_does_not_undo_it() {
    let states = Arc::new(SlowStates::default());
    let health = Arc::new(crate::sender::health::Health::new(states.clone()));
    *states.disable_delay.lock() = Duration::from_secs(2);
    let endpoint = Configured { endpoint_key: "a".into(), org_id: "org_1".into(), digest: "d".into() };
    let disabling = tokio::spawn({
        let (health, endpoint) = (health.clone(), endpoint.clone());
        async move { health.disable(&endpoint, DisabledReason::Redirect).await }
    });
    // Let the disable start its write before the read.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    health.refresh().await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    disabling.await.unwrap().unwrap();
    assert!(health.disabled("a"), "the disable is kept once its write returns");
}

#[tokio::test]
async fn a_delivery_with_under_15_s_of_claim_left_is_released_unsent() {
    let (url, got, addr) = receiver(StatusCode::OK).await;
    let (queue, states) = (Arc::new(Queue::default()), Arc::new(States::default()));
    let health = Arc::new(crate::sender::health::Health::new(states));
    health.refresh().await.unwrap();
    let send = crate::sender::send::Dispatch {
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
