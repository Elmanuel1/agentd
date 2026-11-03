//! Logs, traces and metrics at agentd's boundaries.
//!
//! - every HTTP request is a span with the route, status and request id,
//!   and feeds `http.server.request.duration` and `http.server.active_requests`
//!   ([`http`]); an SSE response counts in `agentd.sse.active_streams` until
//!   the client goes away,
//! - every SQL statement feeds `db.client.operation.duration` ([`db`]),
//! - every blob, secret and sandbox operation is a span and feeds
//!   `agentd.port.operation.duration` ([`ports`]),
//! - each of these carries `outcome` (`ok` or `error`) or the status code. A
//!   5xx, a failed statement or a failed connection is logged at `error`, a
//!   failed port operation at `warn`.
//!
//! Traces, metrics and logs go over OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set
//! (the standard `OTEL_*` variables apply); logs also go to stdout, as JSON
//! when `AGENTD_LOG_FORMAT=json`.

pub mod db;
pub mod http;
pub mod ports;

use opentelemetry::metrics::{Histogram, Meter, UpDownCounter};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::filter::{filter_fn, LevelFilter};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Histogram buckets in seconds (the OpenTelemetry HTTP semantic conventions'
/// advice). The SDK's defaults are sized for milliseconds, which would put
/// every request below 5 s into the first bucket.
const SECONDS: [f64; 14] = [0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0];

/// The instruments every boundary records into.
#[derive(Debug, Clone)]
pub struct Metrics {
    pub(crate) http_duration: Histogram<f64>,
    pub(crate) http_active: UpDownCounter<i64>,
    pub(crate) sse_active: UpDownCounter<i64>,
    pub(crate) port_duration: Histogram<f64>,
    pub(crate) db_duration: Histogram<f64>,
}

impl Metrics {
    /// Instruments on `meter`.
    pub fn new(meter: &Meter) -> Self {
        Self {
            http_duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_boundaries(SECONDS.to_vec())
                .with_description("HTTP requests agentd served")
                .build(),
            http_active: meter.i64_up_down_counter("http.server.active_requests").build(),
            sse_active: meter
                .i64_up_down_counter("agentd.sse.active_streams")
                .with_description("Open session event streams")
                .build(),
            port_duration: meter
                .f64_histogram("agentd.port.operation.duration")
                .with_unit("s")
                .with_boundaries(SECONDS.to_vec())
                .with_description("Blob, secret and sandbox operations")
                .build(),
            db_duration: meter
                .f64_histogram("db.client.operation.duration")
                .with_unit("s")
                .with_boundaries(SECONDS.to_vec())
                .with_description("SQL statements")
                .build(),
        }
    }

    /// Instruments on the global meter provider.
    pub fn global() -> Self {
        Self::new(&opentelemetry::global::meter("agentd"))
    }
}

/// Flushes and stops the exporters when dropped.
#[derive(Debug, Default)]
pub struct Telemetry {
    tracer: Option<SdkTracerProvider>,
    meter: Option<SdkMeterProvider>,
    logger: Option<SdkLoggerProvider>,
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(t) = self.tracer.take() {
            let _ = t.shutdown();
        }
        if let Some(m) = self.meter.take() {
            let _ = m.shutdown();
        }
        if let Some(l) = self.logger.take() {
            let _ = l.shutdown();
        }
    }
}

/// Install the log subscriber and, when `otlp` is set, the OTLP trace,
/// metric and log pipelines (the meter provider as the global one; spans
/// reach the tracer through the `tracing` layer). Logs
/// still go to stdout; over OTLP they carry the trace and span ids.
pub fn init(otlp: bool, json: bool) -> Result<Telemetry, String> {
    // RUST_LOG picks the logs, on stdout and over OTLP alike; it does not
    // pick the spans, so quieter logs never switch tracing off.
    let log_filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt = if json {
        tracing_subscriber::fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(true)
            .with_filter(log_filter())
            .with_filter(filter_fn(|m| !aws_wire_detail(m)))
            .boxed()
    } else {
        tracing_subscriber::fmt::layer()
            .with_filter(log_filter())
            .with_filter(filter_fn(|m| !aws_wire_detail(m)))
            .boxed()
    };
    let mut telemetry = Telemetry::default();
    let otel = if otlp {
        let resource = Resource::builder().with_service_name("agentd").build();
        let spans = SpanExporter::builder().with_tonic().build().map_err(|e| e.to_string())?;
        let tracer =
            SdkTracerProvider::builder().with_batch_exporter(spans).with_resource(resource.clone()).build();
        let metrics = MetricExporter::builder().with_tonic().build().map_err(|e| e.to_string())?;
        let meter = SdkMeterProvider::builder()
            .with_periodic_exporter(metrics)
            .with_resource(resource.clone())
            .build();
        let logs = LogExporter::builder().with_tonic().build().map_err(|e| e.to_string())?;
        let logger = SdkLoggerProvider::builder().with_batch_exporter(logs).with_resource(resource).build();
        opentelemetry::global::set_meter_provider(meter.clone());
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        let spans = tracing_opentelemetry::layer()
            .with_tracer(tracer.tracer("agentd"))
            .with_filter(LevelFilter::INFO)
            .with_filter(filter_fn(|m| !exporter_internal(m)));
        let records = OpenTelemetryTracingBridge::new(&logger)
            .with_filter(log_filter())
            .with_filter(filter_fn(|m| !exporter_internal(m) && !aws_wire_detail(m)));
        let layer = spans.and_then(records);
        telemetry = Telemetry { tracer: Some(tracer), meter: Some(meter), logger: Some(logger) };
        Some(layer)
    } else {
        None
    };
    tracing_subscriber::registry().with(fmt).with(otel).try_init().map_err(|e| e.to_string())?;
    Ok(telemetry)
}

/// The exporters' own transport logs, which must not feed back into the log
/// exporter.
fn exporter_internal(m: &tracing::Metadata<'_>) -> bool {
    ["opentelemetry", "tonic", "h2", "hyper", "tower"].iter().any(|t| m.target().starts_with(t))
}

/// The AWS SDK's `debug` and `trace` events, which print whole requests and
/// responses (a `PutSecretValue` body is the secret). No log takes them,
/// whatever `RUST_LOG` says.
pub(crate) fn aws_wire_detail(m: &tracing::Metadata<'_>) -> bool {
    m.target().starts_with("aws_") && *m.level() > tracing::Level::INFO
}

/// `ok` or `error`, the attribute every boundary records.
pub(crate) fn outcome<T, E>(r: &Result<T, E>) -> &'static str {
    if r.is_ok() {
        "ok"
    } else {
        "error"
    }
}

#[cfg(test)]
mod redaction_tests;
#[cfg(test)]
mod tests;
