//! The wake, the claim loop and the send path.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::StatusCode;
use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::json;
use shared::ports::broadcast::{Broadcast, LocalBroadcast};
use shared::ports::changes::ChangeHandler;

use super::*;
use crate::queue::{Attempted, Next};
use crate::sign::{SigningSecret, sign};
use crate::states::{DisabledReason, EndpointState};

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
    let secret = SigningSecret::parse("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw").unwrap();
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
        assert!(
            matches!(crate::sender::send::retry_or_drop(2), Next::Retry(w) if (30..=60).contains(&w.as_secs()))
        );
        assert!(
            matches!(crate::sender::send::retry_or_drop(1), Next::Retry(w) if (15..=30).contains(&w.as_secs()))
        );
    }
    assert_eq!(crate::sender::send::retry_or_drop(3), Next::Dropped);
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
async fn a_redirect_disables_its_endpoint_at_once_with_one_error_naming_org_endpoint_and_reason() {
    let errors = Arc::new(Mutex::new(Vec::new()));
    let _capture = tracing::subscriber::set_default(Errors(errors.clone()));
    let (url, _, addr) = receiver(StatusCode::FOUND).await;
    let rig = start(&url, vec![addr], vec![due("whe_1", &url, 0)]);
    rig.until(|q| !q.recorded.lock().is_empty()).await;
    assert_eq!(rig.states.disabled.lock().as_slice(), [("a".to_owned(), DisabledReason::Redirect)]);
    rig.stop().await;
    let errors = errors.lock();
    assert_eq!(errors.len(), 1, "{errors:?}");
    for field in ["org=org_1", "endpoint=a", "reason=\"redirect\""] {
        assert!(errors[0].contains(field), "{field} in {}", errors[0]);
    }
    assert!(!errors[0].contains(&url), "the URL stays out of the log");
}

/// Each `ERROR` event's fields, as `name=value` pairs.
struct Errors(Arc<Mutex<Vec<String>>>);

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl tracing::Subscriber for Errors {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::ERROR {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0.lock().push(fields.0);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn a_delivery_whose_claims_lapsed_three_times_or_whose_endpoint_is_disabled_drops_unsent() {
    let (url, got, addr) = receiver(StatusCode::OK).await;
    let rig = start(&url, vec![addr], vec![due("whe_3", &url, 3)]);
    rig.until(|q| !q.dropped.lock().is_empty()).await;
    let disabled =
        EndpointState { endpoint_key: "a".into(), failing: false, disabled: Some(DisabledReason::Failing) };
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
