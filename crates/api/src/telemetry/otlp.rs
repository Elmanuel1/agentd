//! The OTLP exporters, each reached over TLS when its endpoint is `https://`:
//! the collector's certificate chains to `OTEL_EXPORTER_OTLP_CERTIFICATE` when
//! that is set, else to the operating system's roots (the exporter
//! specification's setting, which the Rust exporter does not read itself). A
//! plain or `unix:` endpoint gets no TLS configuration, which tonic refuses
//! for it.

use opentelemetry_otlp::tonic_types::transport::{Certificate, ClientTlsConfig};
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithTonicConfig};

use crate::config::TelemetryConfig;

/// The trace, metric and log exporters `config` describes.
pub(crate) fn exporters(
    config: &TelemetryConfig,
) -> Result<(SpanExporter, MetricExporter, LogExporter), String> {
    let https = config.otlp_https;
    let tls = match https.traces || https.metrics || https.logs {
        true => Some(tls(config.otlp_certificate.as_deref())?),
        false => None,
    };
    let over = |https: bool| tls.clone().filter(|_| https);
    let spans = SpanExporter::builder().with_tonic();
    let spans = match over(https.traces) {
        Some(tls) => spans.with_tls_config(tls),
        None => spans,
    };
    let metrics = MetricExporter::builder().with_tonic();
    let metrics = match over(https.metrics) {
        Some(tls) => metrics.with_tls_config(tls),
        None => metrics,
    };
    let logs = LogExporter::builder().with_tonic();
    let logs = match over(https.logs) {
        Some(tls) => logs.with_tls_config(tls),
        None => logs,
    };
    let built = (spans.build(), metrics.build(), logs.build());
    match built {
        (Ok(spans), Ok(metrics), Ok(logs)) => Ok((spans, metrics, logs)),
        (Err(e), ..) | (_, Err(e), _) => Err(e.to_string()),
        (.., Err(e)) => Err(e.to_string()),
    }
}

/// The TLS of an `https://` exporter.
pub(crate) fn tls(certificate: Option<&str>) -> Result<ClientTlsConfig, String> {
    match certificate {
        Some(path) => {
            let pem =
                std::fs::read(path).map_err(|e| format!("OTEL_EXPORTER_OTLP_CERTIFICATE {path}: {e}"))?;
            Ok(ClientTlsConfig::new().ca_certificate(Certificate::from_pem(pem)))
        }
        None => Ok(ClientTlsConfig::new().with_native_roots()),
    }
}

#[cfg(test)]
mod tests;
