//! The service's credential reads against Secrets Manager on LocalStack: a
//! value changed in the store applies to the next request with no restart,
//! and a store that cannot be reached refuses the request. They run when
//! `AGENTD_AWS_ENDPOINT` names the emulator.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use axum::http::HeaderMap;
use egress::source::VaultSource;
use egress_proxy::{serve, BindingSpec, Registration, Registry};
use secrets_aws::{AwsSecretReader, AwsSecretStore, AwsSecretsConfig, SecretKey, SecretStore};
use tokio::net::TcpListener;
use vaults::types::{CredentialAuthInput, CredentialNetworking, SecretString};

const PASSWORD: &str = "agentd-px-localstack";

fn sealed(value: &str) -> secrets_aws::SecretString {
    CredentialAuthInput::EnvironmentVariable {
        secret_name: "ECHO_TOKEN".into(),
        secret_value: SecretString::new(value),
        networking: Some(CredentialNetworking::Limited { allowed_hosts: vec!["localhost".into()] }),
        injection_location: None,
    }
    .seal()
}

async fn store(endpoint: &str, prefix: &str) -> AwsSecretStore {
    let shared = aws_common::sdk_config(None, None).await;
    let kms = aws_sdk_kms::Client::from_conf(
        aws_sdk_kms::config::Builder::from(&shared).endpoint_url(endpoint).build(),
    );
    let key = kms.create_key().send().await.unwrap().key_metadata.unwrap().key_id;
    let mut config = AwsSecretsConfig::new(prefix, key);
    config.endpoint = Some(endpoint.to_owned());
    AwsSecretStore::connect(&config).await.unwrap()
}

/// An upstream that records the Authorization header of each request.
async fn upstream() -> (u16, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move |h: HeaderMap| {
            let auth = h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
            record.lock().unwrap().push(auth);
            async { "ok" }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (port, seen)
}

/// A proxy reading through `reader`, with session `sesn_1` holding the
/// credential at `key`.
async fn proxy(reader: AwsSecretReader, key: &SecretKey) -> u16 {
    let registry = Arc::new(Registry::new(vec!["localhost".into()]));
    let digest = hex_digest(PASSWORD);
    registry
        .register(
            "sesn_1",
            Registration {
                password_sha256: digest,
                allowed_hosts: None,
                credentials: vec![BindingSpec {
                    id: key.credential().into(),
                    name: "ECHO_TOKEN".into(),
                    placeholder: "agentd-ph-echo".into(),
                    workspace: key.workspace().into(),
                    vault: key.vault().into(),
                }],
            },
        )
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(serve(listener, registry, Arc::new(VaultSource(reader))));
    port
}

fn hex_digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

async fn call(proxy: u16, up: u16) -> reqwest::StatusCode {
    let p = reqwest::Proxy::all(format!("http://127.0.0.1:{proxy}")).unwrap().basic_auth("sesn_1", PASSWORD);
    let client = reqwest::Client::builder().proxy(p).build().unwrap();
    let sent = client.get(format!("http://localhost:{up}/")).header("authorization", "Bearer agentd-ph-echo");
    sent.send().await.unwrap().status()
}

fn key() -> SecretKey {
    let id = uuid::Uuid::new_v4().simple().to_string();
    SecretKey::new("wrkspc_egress", &format!("vlt_{}", id.get(..8).unwrap()), &format!("vcrd_{id}")).unwrap()
}

#[tokio::test]
async fn a_value_changed_in_secrets_manager_applies_to_the_next_request() {
    let Ok(endpoint) = std::env::var("AGENTD_AWS_ENDPOINT") else { return };
    let prefix = format!("egress{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let store = store(&endpoint, &prefix).await;
    let key = key();
    store.put(&key, &sealed("first-value-0001")).await.unwrap();
    let reader = AwsSecretReader::connect(&prefix, None, Some(&endpoint), None).await.unwrap();
    let (up, seen) = upstream().await;
    let port = proxy(reader, &key).await;
    assert_eq!(call(port, up).await, 200);
    store.put(&key, &sealed("second-value-0002")).await.unwrap();
    assert_eq!(call(port, up).await, 200);
    assert_eq!(*seen.lock().unwrap(), ["Bearer first-value-0001", "Bearer second-value-0002"]);
}

#[tokio::test]
async fn an_unreachable_secrets_manager_refuses_the_request() {
    let Ok(_) = std::env::var("AGENTD_AWS_ENDPOINT") else { return };
    // Nothing listens on port 9: every read fails.
    let reader = AwsSecretReader::connect("agentd", Some("us-east-1"), Some("http://127.0.0.1:9"), None)
        .await
        .unwrap();
    let (up, seen) = upstream().await;
    let port = proxy(reader, &key()).await;
    assert_eq!(call(port, up).await, 502);
    assert!(seen.lock().unwrap().is_empty(), "the request reached the upstream");
}
