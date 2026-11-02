//! Telemetry settings: OTLP export and the log format.

use super::{selector, ConfigError};

/// Where logs, traces and metrics go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TelemetryConfig {
    /// Export traces, metrics and logs over OTLP (`OTEL_EXPORTER_OTLP_ENDPOINT`
    /// is set; the OpenTelemetry SDK reads the `OTEL_*` variables itself).
    pub otlp: bool,
    /// Logs as JSON lines (`AGENTD_LOG_FORMAT=json`), else text.
    pub json_logs: bool,
}

impl TelemetryConfig {
    pub(super) fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let otlp = get("OTEL_EXPORTER_OTLP_ENDPOINT").is_some_and(|e| !e.is_empty());
        let json_logs = match selector(get, "AGENTD_LOG_FORMAT", "text").as_str() {
            "text" => false,
            "json" => true,
            other => {
                return Err(ConfigError {
                    var: "AGENTD_LOG_FORMAT",
                    reason: format!("unknown format `{other}`"),
                })
            }
        };
        Ok(Self { otlp, json_logs })
    }
}
