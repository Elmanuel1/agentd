//! A delivery's headers, signature and attempts against a local receiver.

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use parking_lot::Mutex;
use serde_json::json;

use super::*;
use crate::sign::SigningSecret;

#[derive(Default)]
struct Marks(Mutex<Vec<(String, i64)>>);

impl DeliveryMarksImpl for Marks {
    async fn delivered(&self, id: &str, _: &str, attempt: i64) -> Result<(), BackendError> {
        self.0.lock().push((format!("delivered {id}"), attempt));
        Ok(())
    }
    async fn dropped(&self, id: &str, _: &str, attempt: i64) -> Result<(), BackendError> {
        self.0.lock().push((format!("dropped {id}"), attempt));
        Ok(())
    }
}

async fn receiver(code: StatusCode) -> (String, Arc<Mutex<Vec<HeaderMap>>>, Arc<AtomicUsize>) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let n = Arc::new(AtomicUsize::new(0));
    let (seen, count) = (got.clone(), n.clone());
    let app = axum::Router::new().route(
        "/hook",
        post(move |h: HeaderMap| async move {
            seen.lock().push(h);
            count.fetch_add(1, Ordering::SeqCst);
            code
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    (url, got, n)
}

fn change(url: &str) -> Change {
    Change {
        table: "webhook_deliveries".into(),
        op: ChangeOp::Insert,
        position: None,
        before: None,
        after: Some(json!({"webhook_id": "whe_1", "endpoint_key": "a", "endpoint_url": url, "body": "{}"})),
    }
}

fn hooks(url: &str) -> Arc<Webhooks> {
    let json = json!([{"id": "a", "organization_id": "org_1", "url": url, "secret": "whsec_c2VjcmV0"}]);
    Arc::new(Webhooks::parse(&json.to_string()).unwrap())
}

#[tokio::test]
async fn a_delivery_is_signed_and_sent_with_live_s_headers() {
    let (url, got, _) = receiver(StatusCode::OK).await;
    let marks = Arc::new(Marks::default());
    let sender = WebhookSender::new(hooks(&url), marks.clone(), Duration::from_millis(10)).unwrap();
    sender.handle(&change(&url)).await.unwrap();
    let h = got.lock()[0].clone();
    assert_eq!(h["user-agent"], "Claude-Platform");
    assert_eq!(h["webhook-id"], "whe_1");
    let ts: i64 = h["webhook-timestamp"].to_str().unwrap().parse().unwrap();
    let secret = SigningSecret::parse("whsec_c2VjcmV0").unwrap();
    assert_eq!(h["webhook-signature"].to_str().unwrap(), sign(&secret, "whe_1", ts, "{}"));
    assert_eq!(marks.0.lock().as_slice(), [("delivered whe_1".to_owned(), 1)]);
}

#[tokio::test]
async fn a_refused_delivery_gets_three_attempts_with_one_id_then_drops() {
    let (url, got, n) = receiver(StatusCode::UNAUTHORIZED).await;
    let marks = Arc::new(Marks::default());
    let sender = WebhookSender::new(hooks(&url), marks.clone(), Duration::from_millis(10)).unwrap();
    sender.handle(&change(&url)).await.unwrap();
    for _ in 0..100 {
        if !marks.0.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(n.load(Ordering::SeqCst), 3);
    assert!(got.lock().iter().all(|h| h["webhook-id"] == "whe_1"));
    assert_eq!(marks.0.lock().as_slice(), [("dropped whe_1".to_owned(), 3)]);
}
