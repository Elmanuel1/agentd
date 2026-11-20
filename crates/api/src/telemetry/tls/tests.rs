//! Spans exported to an `https://` collector: a [`TlsFront`] serving the test
//! certificate (`docker/tls/certs.sh`), its CA at `AGENTD_OTLP_TLS_CA`; skipped without it.

use std::path::Path;
use std::time::Duration;

use opentelemetry::trace::{Tracer, TracerProvider};
use opentelemetry_otlp::{SpanExporter, WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::trace::SdkTracerProvider;
use test_support::TlsFront;

use super::otlp;

/// How many TLS handshakes the front saw after one span was exported to it
/// with `certificate` trusted.
async fn handshakes(ca: &str, certificate: Option<&str>) -> usize {
    let front = TlsFront::start(Path::new(ca), None).await;
    let exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(format!("https://localhost:{}", front.addr.port()))
        .with_timeout(Duration::from_secs(1))
        .with_tls_config(otlp(certificate).unwrap())
        .build()
        .unwrap();
    let provider = SdkTracerProvider::builder().with_batch_exporter(exporter).build();
    provider.tracer("test").in_span("exported", |_| {});
    tokio::task::spawn_blocking(move || drop(provider.force_flush())).await.unwrap();
    front.handshakes()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_https_collector_signed_by_the_certificate_gets_the_spans() {
    let Ok(ca) = std::env::var("AGENTD_OTLP_TLS_CA") else { return };
    assert_eq!(handshakes(&ca, Some(&ca)).await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_https_collector_the_roots_do_not_sign_is_refused() {
    let Ok(ca) = std::env::var("AGENTD_OTLP_TLS_CA") else { return };
    assert_eq!(handshakes(&ca, None).await, 0);
}

#[test]
fn a_certificate_that_cannot_be_read_is_an_error() {
    let err = otlp(Some("/nonexistent/ca.pem")).unwrap_err();
    assert!(err.starts_with("OTEL_EXPORTER_OTLP_CERTIFICATE /nonexistent/ca.pem: "), "{err}");
}
