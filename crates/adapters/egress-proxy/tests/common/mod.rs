//! What the proxy tests share: a credential store, an upstream, and a proxy
//! with one session registered through the admin interface.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use egress_proxy::{
    admin, serve, BindingSpec, Current, Locked, Registered, Registration, Registry, SecretSource,
};
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use parking_lot::Mutex;
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use shared::ports::secrets::{SecretError, SecretKey};
use tokio::net::TcpListener;

pub const SECRET: &str = "ghp_real_value_1";
pub const ADMIN_TOKEN: &str = "admin-token-for-tests";

/// The credential store: its value can change, or it can be unreachable.
pub struct Vault(pub Mutex<Option<String>>);

#[async_trait]
impl SecretSource for Vault {
    async fn current(&self, key: &SecretKey) -> Result<Current, SecretError> {
        let value = self.0.lock().clone().ok_or_else(|| SecretError::Backend("unreachable".into()))?;
        // The model key is bound to 127.0.0.1, every other credential to localhost.
        let host = if key.credential() == "vcrd_model" { "127.0.0.1" } else { "localhost" };
        Ok(Current {
            value: Locked::new(value.as_bytes()).unwrap(),
            hosts: Some(vec![host.into()]),
            header: true,
        })
    }
}
/// An upstream that records the Authorization it got and echoes it back.
pub async fn upstream(seen: Arc<Mutex<Vec<String>>>) -> u16 {
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
                        let mut r = hyper::Response::new(Full::new(Bytes::from(format!("you sent {auth}"))));
                        let echo = hyper::header::HeaderValue::from_str(&auth).unwrap();
                        r.headers_mut().insert("x-echo", echo);
                        Ok::<_, Infallible>(r)
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

pub struct Proxy {
    pub port: u16,
    pub ca: String,
    pub vault: Arc<Vault>,
}

/// A proxy with one session, registered through the admin interface.
pub async fn proxy() -> Proxy {
    let registry = Arc::new(Registry::new(vec!["localhost".into(), "127.0.0.1".into()]));
    let admin_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let admin_port = admin_listener.local_addr().unwrap().port();
    let router = admin::router(registry.clone(), &SecretString::from(ADMIN_TOKEN));
    tokio::spawn(async move { axum::serve(admin_listener, router).await });
    let reg = Registration {
        password_sha256: hex::encode(Sha256::digest(b"agentd-px-good")),
        allowed_hosts: Some(vec!["localhost".into(), "127.0.0.1".into()]),
        credentials: vec![
            BindingSpec {
                id: "vcrd_1".into(),
                name: "GITHUB_TOKEN".into(),
                placeholder: "agentd-ph-abc".into(),
                workspace: "wrkspc_1".into(),
                vault: "vlt_1".into(),
            },
            BindingSpec {
                id: "vcrd_model".into(),
                name: "OPENROUTER_API_KEY".into(),
                placeholder: "agentd-ph-model".into(),
                workspace: "operator".into(),
                vault: "models".into(),
            },
        ],
    };
    let url = format!("http://127.0.0.1:{admin_port}/v1/sessions/sesn_1");
    let http = reqwest::Client::new();
    let refused = http.put(&url).bearer_auth("wrong").json(&reg).send().await.unwrap();
    assert_eq!(refused.status(), 401);
    let done: Registered =
        http.put(&url).bearer_auth(ADMIN_TOKEN).json(&reg).send().await.unwrap().json().await.unwrap();
    let vault = Arc::new(Vault(Mutex::new(Some(SECRET.into()))));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(serve(listener, registry, vault.clone()));
    Proxy { port, ca: done.ca_cert, vault }
}

pub fn client(proxy_port: u16, password: &str, ca: Option<&str>) -> reqwest::Client {
    let proxy =
        reqwest::Proxy::all(format!("http://127.0.0.1:{proxy_port}")).unwrap().basic_auth("sesn_1", password);
    let mut b = reqwest::Client::builder().proxy(proxy);
    if let Some(ca) = ca {
        b = b.add_root_certificate(reqwest::Certificate::from_pem(ca.as_bytes()).unwrap());
    }
    b.build().unwrap()
}
