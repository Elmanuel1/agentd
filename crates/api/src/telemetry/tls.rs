//! The TLS an `https://` collector is reached with: its certificate chains to
//! `OTEL_EXPORTER_OTLP_CERTIFICATE` when that is set, else to the operating
//! system's roots (the OpenTelemetry exporter specification's setting, which
//! the Rust exporter does not read itself).

use opentelemetry_otlp::tonic_types::transport::{Certificate, ClientTlsConfig};

/// The TLS of every OTLP exporter; tonic uses it only for `https://`.
pub(crate) fn otlp(certificate: Option<&str>) -> Result<ClientTlsConfig, String> {
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
