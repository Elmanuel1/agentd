//! The proxy end to end: a client through it, a local upstream behind it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

mod common;

use std::sync::Arc;

use common::{client, proxy, upstream, SECRET};
use parking_lot::Mutex;

#[tokio::test]
async fn an_allowed_host_gets_the_secret_and_the_sandbox_gets_it_masked_back() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    let r = client(p.port, "agentd-px-good", None)
        .get(format!("http://localhost:{up}/x"))
        .header("authorization", "Bearer agentd-ph-abc")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["x-echo"], "Bearer agentd-ph-abc");
    assert_eq!(r.text().await.unwrap(), "you sent Bearer agentd-ph-abc");
    assert_eq!(seen.lock().clone(), [format!("Bearer {SECRET}")]);
}

#[tokio::test]
async fn a_placeholder_for_another_host_and_a_host_off_the_list_are_blocked() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    let c = client(p.port, "agentd-px-good", None);
    let wrong =
        c.get(format!("http://127.0.0.1:{up}/x")).header("x-key", "agentd-ph-abc").send().await.unwrap();
    assert_eq!(wrong.status(), 403);
    let off = c.get("http://example.com/").send().await.unwrap();
    assert_eq!(off.status(), 403);
    assert!(seen.lock().is_empty());
}

#[tokio::test]
async fn a_placeholder_in_the_body_goes_on_only_with_a_credential_the_host_allows() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    let c = client(p.port, "agentd-px-good", None);
    let context = "tool output: GITHUB_TOKEN=agentd-ph-abc";
    let model = c
        .post(format!("http://127.0.0.1:{up}/v1/chat/completions"))
        .header("authorization", "Bearer agentd-ph-model")
        .body(context)
        .send()
        .await
        .unwrap();
    assert_eq!(model.status(), 200);
    let bare = c.post(format!("http://127.0.0.1:{up}/x")).body(context).send().await.unwrap();
    assert_eq!(bare.status(), 403);
    assert_eq!(seen.lock().clone(), [format!("Bearer {SECRET}")]);
}

#[tokio::test]
async fn an_unknown_proxy_password_is_refused() {
    let p = proxy().await;
    let r = client(p.port, "nope", None).get("http://localhost/").send().await.unwrap();
    assert_eq!(r.status(), 407);
}

#[tokio::test]
async fn https_is_decrypted_with_the_sessions_ca() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    // The upstream speaks plain HTTP, so the proxy's own TLS hop fails: a
    // 502 from the proxy, read over TLS the session CA signed, shows the
    // sandbox's leg was decrypted.
    let r = client(p.port, "agentd-px-good", Some(&p.ca))
        .get(format!("https://localhost:{up}/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 502);
}

#[tokio::test]
async fn a_changed_value_applies_to_the_next_request() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    let c = client(p.port, "agentd-px-good", None);
    let call =
        || c.get(format!("http://localhost:{up}/x")).header("authorization", "Bearer agentd-ph-abc").send();
    assert_eq!(call().await.unwrap().status(), 200);
    *p.vault.0.lock() = Some("ghp_rotated_value_2".into());
    assert_eq!(call().await.unwrap().status(), 200);
    assert_eq!(seen.lock().clone(), [format!("Bearer {SECRET}"), "Bearer ghp_rotated_value_2".into()]);
}

#[tokio::test]
async fn an_unreachable_store_refuses_the_request() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let p = proxy().await;
    *p.vault.0.lock() = None;
    let c = client(p.port, "agentd-px-good", None);
    let r = c.get(format!("http://localhost:{up}/x")).header("authorization", "Bearer agentd-ph-abc").send();
    let r = r.await.unwrap();
    assert_eq!(r.status(), 502);
    assert!(r.text().await.unwrap().contains("the request was not sent"));
    // A request that needs no credential still goes through.
    assert_eq!(c.get(format!("http://localhost:{up}/x")).send().await.unwrap().status(), 200);
    assert_eq!(seen.lock().clone(), [String::new()]);
}
