//! The endpoint states: a stale or late read stops posts, and a claim near its end is released.

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use parking_lot::Mutex;
use shared::Timestamp;

use super::*;
use crate::states::{Configured, DisabledReason, EndpointState, NativeEndpointStates};

/// States whose read waits `delay` and fails while `failing`.
#[derive(Default)]
struct SlowStates {
    inner: States,
    delay: Mutex<Duration>,
    failing: std::sync::atomic::AtomicBool,
    clears_fail: std::sync::atomic::AtomicBool,
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
        if self.clears_fail.load(Ordering::SeqCst) {
            return Err(BackendError::new(std::io::Error::other("down")));
        }
        NativeEndpointStates::clear_failing(&self.inner, k, s).await
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

#[tokio::test]
async fn a_run_of_failures_that_cannot_be_ended_keeps_the_read_and_is_ended_by_the_next() {
    let states = Arc::new(SlowStates::default());
    let since = Timestamp::now();
    let row = EndpointState { endpoint_key: "a".into(), failing_since: Some(since), disabled: None };
    states.inner.rows.lock().push(row);
    let health = crate::sender::health::Health::new(states.clone());
    tokio::time::sleep(Duration::from_millis(5)).await;
    health.delivered("a").await.unwrap();
    states.clears_fail.store(true, Ordering::SeqCst);
    health.refresh().await.unwrap();
    assert!(health.fresh(), "the read is kept: posts go on");
    states.clears_fail.store(false, Ordering::SeqCst);
    health.refresh().await.unwrap();
    assert!(health.fresh());
}
