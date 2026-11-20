//! Telemetry settings: OTLP export and the log format.

use super::{ConfigError, selector};

/// Where logs, traces and metrics go.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelemetryConfig {
    /// Export traces, metrics and logs over OTLP (`OTEL_EXPORTER_OTLP_ENDPOINT`
    /// is set; the OpenTelemetry SDK reads the `OTEL_*` variables itself).
    pub otlp: bool,
    /// `OTEL_EXPORTER_OTLP_CERTIFICATE`: the PEM file an `https://`
    /// collector's certificate chains to. Unset, the operating system's roots.
    pub otlp_certificate: Option<String>,
    /// Logs as JSON lines (`AGENTD_LOG_FORMAT=json`), else text.
    pub json_logs: bool,
}

impl TelemetryConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let endpoint = get("OTEL_EXPORTER_OTLP_ENDPOINT").filter(|e| !e.is_empty());
        let otlp_certificate = get("OTEL_EXPORTER_OTLP_CERTIFICATE").filter(|c| !c.is_empty());
        if otlp_certificate.is_some() && !endpoint.as_deref().is_some_and(|e| e.starts_with("https://")) {
            return Err(ConfigError {
                var: "OTEL_EXPORTER_OTLP_CERTIFICATE",
                reason: "needs an https:// OTEL_EXPORTER_OTLP_ENDPOINT".into(),
            });
        }
        let json_logs = match selector(get, "AGENTD_LOG_FORMAT", "text").as_str() {
            "text" => false,
            "json" => true,
            other => {
                return Err(ConfigError {
                    var: "AGENTD_LOG_FORMAT",
                    reason: format!("unknown format `{other}`"),
                });
            }
        };
        Ok(Self { otlp: endpoint.is_some(), otlp_certificate, json_logs })
    }
}
