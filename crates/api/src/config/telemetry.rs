//! Telemetry settings: OTLP export and the log format.

use super::{ConfigError, selector};

/// Where logs, traces and metrics go.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TelemetryConfig {
    /// Export traces, metrics and logs over OTLP (`OTEL_EXPORTER_OTLP_ENDPOINT`
    /// is set; the OpenTelemetry SDK reads the `OTEL_*` variables itself).
    pub otlp: bool,
    /// Which exporters reach an `https://` collector.
    pub otlp_https: OtlpHttps,
    /// `OTEL_EXPORTER_OTLP_CERTIFICATE`: the PEM file an `https://`
    /// collector's certificate chains to. Unset, the operating system's roots.
    pub otlp_certificate: Option<String>,
    /// Logs as JSON lines (`AGENTD_LOG_FORMAT=json`), else text.
    pub json_logs: bool,
}

/// Per exporter, whether its endpoint is `https://`: the signal's own
/// `OTEL_EXPORTER_OTLP_<SIGNAL>_ENDPOINT`, else `OTEL_EXPORTER_OTLP_ENDPOINT`,
/// as the exporter picks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OtlpHttps {
    /// The trace exporter's.
    pub traces: bool,
    /// The metric exporter's.
    pub metrics: bool,
    /// The log exporter's.
    pub logs: bool,
}

impl TelemetryConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let set = |var: &str| get(var).filter(|v| !v.is_empty());
        let endpoint = set("OTEL_EXPORTER_OTLP_ENDPOINT");
        let https = |signal: &str| {
            let own = set(&format!("OTEL_EXPORTER_OTLP_{signal}_ENDPOINT"));
            own.or_else(|| endpoint.clone()).is_some_and(|e| e.starts_with("https://"))
        };
        let otlp_https =
            OtlpHttps { traces: https("TRACES"), metrics: https("METRICS"), logs: https("LOGS") };
        let otlp_certificate = set("OTEL_EXPORTER_OTLP_CERTIFICATE");
        let every_https = otlp_https.traces && otlp_https.metrics && otlp_https.logs;
        if otlp_certificate.is_some() && !every_https {
            return Err(ConfigError {
                var: "OTEL_EXPORTER_OTLP_CERTIFICATE",
                reason: "needs every OTLP endpoint https://".into(),
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
        Ok(Self { otlp: endpoint.is_some(), otlp_https, otlp_certificate, json_logs })
    }
}
