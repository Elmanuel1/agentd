//! The proxy end to end: a client through it, a local upstream behind it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use egress_proxy::{serve, CaPem, Credential, EgressPolicy, SessionCa, SessionEgress};
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use tokio::net::TcpListener;

struct Policy(Arc<SessionEgress>);

#[async_trait]
impl EgressPolicy for Policy {
    async fn session(&self, password: &str) -> Option<Arc<SessionEgress>> {
        (password == "agentd-px-good").then(|| self.0.clone())
    }
}

/// An upstream that records the Authorization it got and echoes it back.
async fn upstream(seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let seen = seen.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let seen = seen.clone();
                    async move {
                        let auth = req
                            .headers()
                            .get("authorization")
                            .map(|v| v.to_str().unwrap().to_owned())
                            .unwrap_or_default();
                        seen.lock().push(auth.clone());
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(format!(
                            "you sent {auth}"
                        )))))
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    port
}

async fn proxy() -> (u16, String) {
    let pem = CaPem::generate("sesn_1").unwrap();
    let session = SessionEgress {
        session: "sesn_1".into(),
        allowed_hosts: Some(vec!["localhost".into(), "127.0.0.1".into()]),
        credentials: vec![Credential {
            id: "vcrd_1".into(),
            name: "GITHUB_TOKEN".into(),
            placeholder: "agentd-ph-abc".into(),
            value: "ghp_real".into(),
            hosts: Some(vec!["localhost".into()]),
            header: true,
            body: false,
        }],
        ca: Arc::new(SessionCa::from_pem(&pem).unwrap()),
        internal_hosts: vec!["localhost".into(), "127.0.0.1".into()],
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(serve(listener, Arc::new(Policy(Arc::new(session)))));
    (port, pem.cert)
}

fn client(proxy_port: u16, password: &str, ca: Option<&str>) -> reqwest::Client {
    let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}"))
        .unwrap()
        .basic_auth("session", password);
    let mut b = reqwest::Client::builder().proxy(proxy);
    if let Some(ca) = ca {
        b = b.add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap());
    }
    b.build().unwrap()
}

#[tokio::test]
async fn an_allowed_host_gets_the_secret_and_the_sandbox_gets_it_masked_back() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let (port, _) = proxy().await;
    let r = client(port, "agentd-px-good", None)
        .get(format!("http://localhost:{up}/x"))
        .header("authorization", "Bearer agentd-ph-abc")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.text().await.unwrap(), "you sent Bearer agentd-ph-abc");
    assert_eq!(seen.lock().clone(), ["Bearer ghp_real"]);
}

#[tokio::test]
async fn a_placeholder_for_another_host_and_a_host_off_the_list_are_blocked() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let (port, _) = proxy().await;
    let c = client(port, "agentd-px-good", None);
    let wrong =
        c.get(format!("http://127.0.0.1:{up}/x")).header("x-key", "agentd-ph-abc").send().await.unwrap();
    assert_eq!(wrong.status(), 403);
    let off = c.get("http://example.com/").send().await.unwrap();
    assert_eq!(off.status(), 403);
    assert!(seen.lock().is_empty());
}

#[tokio::test]
async fn an_unknown_proxy_password_is_refused() {
    let (port, _) = proxy().await;
    let r = client(port, "nope", None).get("http://localhost/").send().await.unwrap();
    assert_eq!(r.status(), 407);
}

#[tokio::test]
async fn https_is_decrypted_with_the_sessions_ca() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let up = upstream(seen.clone()).await;
    let (port, ca) = proxy().await;
    // The upstream speaks plain HTTP, so the proxy's own TLS hop fails: a
    // 502 from the proxy, read over TLS the session CA signed, shows the
    // sandbox's leg was decrypted.
    let r = client(port, "agentd-px-good", Some(&ca))
        .get(format!("https://localhost:{up}/x"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 502);
}
