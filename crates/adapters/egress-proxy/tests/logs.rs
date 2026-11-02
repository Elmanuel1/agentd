//! No secret value reaches a log line, at any level, whatever happens to the
//! request. Its own binary: the subscriber is global, and nothing else runs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::sync::Arc;

use common::{client, proxy, upstream, SECRET};
use parking_lot::Mutex;

/// A log sink the test reads back.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn no_secret_value_reaches_a_log() {
    let logs = Captured::default();
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || sink.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    let c = client(p.port, "agentd-px-good", None);
    let ok = c.get(format!("http://localhost:{up}/x")).header("authorization", "Bearer agentd-ph-abc");
    assert_eq!(ok.send().await.unwrap().status(), 200);
    let wrong = c.get(format!("http://127.0.0.1:{up}/x")).header("x-key", "agentd-ph-abc");
    assert_eq!(wrong.send().await.unwrap().status(), 403);
    *p.vault.0.lock() = None;
    let down = c.get(format!("http://localhost:{up}/x")).header("authorization", "Bearer agentd-ph-abc");
    assert_eq!(down.send().await.unwrap().status(), 502);
    let text = String::from_utf8(logs.0.lock().clone()).unwrap();
    assert!(text.contains("secret injected") && text.contains("secret masked in a response"), "{text}");
    assert!(text.contains("request refused") && text.contains("credential store unreadable"), "{text}");
    assert!(!text.contains(SECRET), "a secret value reached the log");
}
