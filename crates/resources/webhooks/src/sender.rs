//! Sending a delivery: the change feed hands each new `webhook_deliveries`
//! row here, and its first attempt runs then. A delivery not answered 2xx is
//! sent again twice, about 31 s apart, on a task of its own so the feed keeps
//! moving, each attempt signed anew with its own `webhook-timestamp` and the
//! same `webhook-id`, then dropped (H-3). The first attempt is delivered at
//! least once; a replica that stops during the retries leaves the row with
//! neither `delivered_at` nor `dropped_at`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use shared::ports::changes::{Change, ChangeHandler, ChangeOp};
use shared::{BackendError, Timestamp};

use super::endpoints::Webhooks;
use super::sign::sign;

/// The attempts a delivery gets.
const ATTEMPTS: i64 = 3;
/// How long one attempt may take.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Deliveries retried at once per replica.
const RETRYING: usize = 256;

/// Where a delivery's end is recorded.
#[async_trait]
pub trait DeliveryMarks: Send + Sync {
    /// The delivery was answered 2xx on attempt `attempt`.
    async fn delivered(&self, webhook_id: &str, endpoint_key: &str, attempt: i64)
        -> Result<(), BackendError>;
    /// The delivery was dropped after `attempt` attempts.
    async fn dropped(&self, webhook_id: &str, endpoint_key: &str, attempt: i64) -> Result<(), BackendError>;
}

/// Posts deliveries to their endpoints.
#[derive(Clone)]
pub struct WebhookSender {
    http: reqwest::Client,
    webhooks: Arc<Webhooks>,
    marks: Arc<dyn DeliveryMarks>,
    spacing: Duration,
    retrying: Arc<tokio::sync::Semaphore>,
}

impl std::fmt::Debug for WebhookSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookSender").field("spacing", &self.spacing).finish_non_exhaustive()
    }
}

impl WebhookSender {
    /// A sender to `webhooks`' endpoints, recording each end in `marks`,
    /// attempts `spacing` apart (live: about 31 s).
    pub fn new(
        webhooks: Arc<Webhooks>,
        marks: Arc<dyn DeliveryMarks>,
        spacing: Duration,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent("Claude-Platform")
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { http, webhooks, marks, spacing, retrying: Arc::new(tokio::sync::Semaphore::new(RETRYING)) })
    }

    async fn attempt(&self, id: &str, url: &str, body: &str, secret: &super::SigningSecret) -> bool {
        let ts = Timestamp::now().as_datetime().timestamp();
        let sent = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .header("webhook-id", id)
            .header("webhook-timestamp", ts.to_string())
            .header("webhook-signature", sign(secret, id, ts, body))
            .body(body.to_owned())
            .send()
            .await;
        sent.is_ok_and(|r| r.status().is_success())
    }
}

#[async_trait]
impl ChangeHandler for WebhookSender {
    fn table(&self) -> &'static str {
        "webhook_deliveries"
    }

    /// A new row is sent; its own marks (updates) are not changes to send.
    /// An endpoint no longer configured drops the delivery.
    async fn handle(&self, change: &Change) -> Result<(), BackendError> {
        let Some(row) = change.after.as_ref().filter(|_| change.op == ChangeOp::Insert) else {
            return Ok(());
        };
        let field = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or_default();
        let (id, key, url, body) =
            (field("webhook_id"), field("endpoint_key"), field("endpoint_url"), field("body"));
        let Some(endpoint) = self.webhooks.endpoint(key) else {
            return self.marks.dropped(id, key, 0).await;
        };
        if self.attempt(id, url, body, &endpoint.secret).await {
            return self.marks.delivered(id, key, 1).await;
        }
        let (me, delivery) = (self.clone(), [id, key, url, body].map(str::to_owned));
        let secret = endpoint.secret.clone();
        tokio::spawn(async move {
            let Ok(_held) = me.retrying.clone().acquire_owned().await else { return };
            let [id, key, url, body] = delivery;
            let outcome = me.retry(&id, &key, &url, &body, &secret).await;
            if let Err(e) = outcome {
                tracing::warn!(webhook_id = %id, error = %e, "webhooks: a delivery's end could not be recorded");
            }
        });
        Ok(())
    }
}

impl WebhookSender {
    /// The attempts after the first, `spacing` apart, then the delivery's end.
    async fn retry(
        &self,
        id: &str,
        key: &str,
        url: &str,
        body: &str,
        secret: &super::SigningSecret,
    ) -> Result<(), BackendError> {
        for attempt in 2..=ATTEMPTS {
            tokio::time::sleep(self.spacing).await;
            if self.attempt(id, url, body, secret).await {
                return self.marks.delivered(id, key, attempt).await;
            }
        }
        tracing::info!(webhook_id = %id, endpoint = %key, "webhooks: a delivery was dropped after its attempts");
        self.marks.dropped(id, key, ATTEMPTS).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use parking_lot::Mutex;
    use serde_json::json;

    use super::*;
    use crate::sign::SigningSecret;

    #[derive(Default)]
    struct Marks(Mutex<Vec<(String, i64)>>);

    #[async_trait]
    impl DeliveryMarks for Marks {
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
            before: None,
            after: Some(
                json!({"webhook_id": "whe_1", "endpoint_key": "a", "endpoint_url": url, "body": "{}"}),
            ),
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
}
