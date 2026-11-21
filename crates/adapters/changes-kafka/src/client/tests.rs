#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rdkafka::client::ClientContext;
use shared::BackendError;
use shared::ports::BoxFuture;
use shared::ports::credentials::{BearerToken, BearerTokenSource};

use super::{Context, Tokens};

/// Fails its first `failing` fetches, then gives tokens valid 15 minutes.
#[derive(Debug)]
struct Flaky {
    failing: usize,
    asked: AtomicUsize,
}

impl BearerTokenSource for Flaky {
    fn token(&self) -> BoxFuture<'_, Result<BearerToken, BackendError>> {
        let n = self.asked.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if n < self.failing {
                return Err(BackendError::new(std::io::Error::other("credentials unavailable")));
            }
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
            let value = format!("t{n}");
            Ok(BearerToken {
                value,
                principal: "agentd".into(),
                expires_at_ms: i64::try_from(now).unwrap() + 900_000,
            })
        })
    }
}

fn context(source: Flaky, cached: Option<BearerToken>) -> Context {
    let tokens = Arc::new(Tokens { source: Arc::new(source), last: Mutex::new(cached) });
    Context { bearer: Some(tokens), runtime: tokio::runtime::Handle::current() }
}

/// librdkafka's asks, each after the wait it takes before asking again.
async fn ask(context: &Context) -> Result<String, String> {
    tokio::time::sleep(Duration::from_millis(50)).await;
    context.generate_oauth_token(None).map(|t| t.token).map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_token_an_ask_is_refused_and_the_next_one_has_the_fetched_token() {
    let context = context(Flaky { failing: 0, asked: AtomicUsize::new(0) }, None);
    assert_eq!(ask(&context).await, Err("no OAUTHBEARER token yet".into()));
    assert_eq!(ask(&context).await, Ok("t0".into()));
}

#[tokio::test(flavor = "multi_thread")]
async fn after_failed_fetches_an_expired_token_recovers() {
    let expired = BearerToken { value: "old".into(), principal: "agentd".into(), expires_at_ms: 1 };
    let context = context(Flaky { failing: 2, asked: AtomicUsize::new(0) }, Some(expired));
    assert!(ask(&context).await.is_err(), "first fetch fails");
    assert!(ask(&context).await.is_err(), "second fetch fails");
    let mut got = ask(&context).await;
    for _ in 0..3 {
        if got.is_ok() {
            break;
        }
        got = ask(&context).await;
    }
    let token = got.unwrap();
    assert!(token == "t2" || token == "t3", "a fresh token, not the expired one: {token}");
}
