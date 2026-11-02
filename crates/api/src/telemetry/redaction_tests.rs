//! No secret value reaches a log line or an error, at any log level: the AWS
//! secret store runs against a fake Secrets Manager and KMS while every event
//! down to `trace` is captured through agentd's log filter, and also without
//! it, to show what the filter stops.

use std::io::Write;
use std::sync::{Arc, Mutex, Once};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bytes::Bytes;
use secrets_aws::{AwsSecretStore, AwsSecretsConfig};
use shared::ports::secrets::{SecretKey, SecretStore, SecretString};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{EnvFilter, Layer};

use super::ports::Observed;
use super::{aws_wire_detail, Metrics};

const SECRET: &str = "sk-live-known-value-7f3a";

/// Answers every call; with `deny_writes`, a write fails with a message that
/// quotes the whole request, the worst an AWS message could do.
async fn fake_aws(deny_writes: bool) -> String {
    let app = Router::new()
        .fallback(move |headers: HeaderMap, body: Bytes| async move { answer(&headers, &body, deny_writes) });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{addr}")
}

fn answer(headers: &HeaderMap, body: &[u8], deny_writes: bool) -> Response {
    let target = headers.get("x-amz-target").and_then(|t| t.to_str().ok()).unwrap_or_default();
    let named =
        r#""ARN":"arn:aws:secretsmanager:us-east-1:111122223333:secret:agentd/w/v/c-AbCdEf","Name":"n""#;
    let (status, json) = match target {
        "TrentService.DescribeKey" => (
            StatusCode::OK,
            r#"{"KeyMetadata":{"KeyId":"k","Arn":"arn:aws:kms:us-east-1:111122223333:key/k",
                "KeyManager":"CUSTOMER","Enabled":true,"KeyState":"Enabled",
                "KeyUsage":"ENCRYPT_DECRYPT","KeySpec":"SYMMETRIC_DEFAULT"}}"#
                .to_owned(),
        ),
        "secretsmanager.CreateSecret" | "secretsmanager.PutSecretValue" if deny_writes => (
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "__type": "AccessDeniedException",
                "message": format!("denied: {}", String::from_utf8_lossy(body)),
            })
            .to_string(),
        ),
        "secretsmanager.GetSecretValue" => {
            (StatusCode::OK, format!(r#"{{{named},"SecretString":"{SECRET}","VersionId":"v1"}}"#))
        }
        _ => (StatusCode::OK, format!(r#"{{{named},"VersionId":"v1"}}"#)),
    };
    (status, [("content-type", "application/x-amz-json-1.1")], json).into_response()
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

/// Every event at `RUST_LOG=trace` into `filtered` through agentd's filter,
/// as text and as JSON with span fields, and into `raw` without it.
fn capture(filtered: &Captured, raw: &Captured) -> tracing::subscriber::DefaultGuard {
    let text = tracing_subscriber::fmt::layer()
        .with_writer(filtered.clone())
        .with_filter(EnvFilter::new("trace"))
        .with_filter(filter_fn(|m| !aws_wire_detail(m)));
    let json = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_writer(filtered.clone())
        .with_filter(EnvFilter::new("trace"))
        .with_filter(filter_fn(|m| !aws_wire_detail(m)));
    let unfiltered =
        tracing_subscriber::fmt::layer().with_writer(raw.clone()).with_filter(EnvFilter::new("trace"));
    tracing::subscriber::set_default(tracing_subscriber::registry().with(text).with(json).with(unfiltered))
}

async fn store(deny_writes: bool) -> Observed<AwsSecretStore> {
    static ENV: Once = Once::new();
    // Static credentials so the default chain never probes a profile or IMDS,
    // and the SDK's own switch for logging secret responses, which an
    // operator could turn on while debugging.
    ENV.call_once(|| {
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("LOG_SENSITIVE_BODIES", "true");
    });
    let mut config = AwsSecretsConfig::new("agentd", "alias/agentd");
    config.region = Some("us-east-1".into());
    config.endpoint = Some(fake_aws(deny_writes).await);
    let store = AwsSecretStore::connect(&config).await.unwrap();
    Observed::new(Arc::new(store), Arc::new(Metrics::global()))
}

fn key() -> SecretKey {
    SecretKey::new("wrkspc_01a", "vlt_01b", "cred_01c").unwrap()
}

#[tokio::test]
async fn a_written_and_read_secret_reaches_no_log_line() {
    let (filtered, raw) = (Captured::default(), Captured::default());
    let _guard = capture(&filtered, &raw);
    let store = store(false).await;
    store.put(&key(), &SecretString::from(SECRET)).await.unwrap();
    let read = store.get(&key()).await.unwrap();
    assert_eq!(format!("{read:?}"), "SecretBox<str>([REDACTED])");
    assert!(raw.text().contains(SECRET), "unfiltered, the SDK logs the GetSecretValue body at trace");
    let logs = filtered.text();
    assert!(logs.contains("port.operation"), "the capture saw the port spans");
    assert!(!logs.contains(SECRET), "a secret reached a log line");
}

#[tokio::test]
async fn a_failed_write_names_the_secret_and_code_but_not_the_value() {
    let (filtered, raw) = (Captured::default(), Captured::default());
    let _guard = capture(&filtered, &raw);
    let store = store(true).await;
    let err = store.put(&key(), &SecretString::from(SECRET)).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "secret backend: secretsmanager:CreateSecret on agentd/wrkspc_01a/vlt_01b/cred_01c failed: \
         AccessDeniedException"
    );
    assert!(!format!("{err:?}").contains(SECRET));
    let logs = filtered.text();
    assert!(logs.contains("port operation failed"), "the failure is logged");
    assert!(!logs.contains(SECRET), "a secret reached a log line");
}
